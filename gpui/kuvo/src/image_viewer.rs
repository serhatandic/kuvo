//! Unified full-window image viewer ("Stage"): the single image viewer used across the entire app
//! (conversation messages, agent `show_file` pictures, inline markdown images, Outputs popover
//! thumbnails and Images drill-down, Files side panel, Work Queue attachments, and Composer
//! attachments).
//!
//! Mounted once on [`crate::Workspace`] and opened from any view via [`open`]. Renders a full-window
//! dark gallery scrim with floating identity/action pills, edge navigation chevrons (for multi-image
//! galleries), adaptive zoom & checkerboard inspection for tiny assets, and a compact bottom
//! filmstrip.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Global, Hsla, Image, ImageFormat,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ObjectFit, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Task, WeakEntity, Window,
    WindowId, anchored, deferred, div, img, point, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Button, ButtonSize, Icon, IconName, IconSize, StyledType, TypeSize,
    menu::Tooltip,
    small::spinner,
    theme::{radius, space},
};

use crate::pictures::{self, Read};

/// Threshold (longest natural side in pixels) at or below which an image is treated as a tiny
/// asset (favicon, icon, badge) and displayed on a transparency checkerboard matte.
const TINY_SIDE: u32 = 96;
/// Maximum number of thumbnails rendered in the bottom filmstrip pill at once.
const FILMSTRIP_WINDOW: usize = 7;
/// Discrete zoom steps for +/- controls.
const ZOOM_STEPS: &[f32] = &[0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];

/// A single image entry presented in the unified Stage viewer.
#[derive(Clone)]
pub struct ViewerItem {
    /// Stable identifier (e.g., conversation picture id, artifact id, file path).
    pub id: String,
    /// Display filename.
    pub name: String,
    /// Optional caption shown beneath the image.
    pub caption: Option<String>,
    /// Short origin tag shown in the top identity pill ("Conversation", "Outputs", "Files", etc.).
    pub source_tag: SharedString,
    /// Optional local file path (enables the "Open in default app" action when on this Mac).
    pub local_path: Option<PathBuf>,
    /// Current read state of this image.
    pub read: Read,
    /// Optional background loader invoked when `read` is [`Read::Reading`] or retried.
    pub loader: Option<Arc<dyn Fn() -> Read + Send + Sync>>,
}

impl ViewerItem {
    /// Returns the natural byte size when already decoded.
    fn byte_size(&self) -> Option<usize> {
        match &self.read {
            Read::Shown { image, .. } => Some(image.bytes().len()),
            _ => None,
        }
    }
}

/// Callback invoked whenever the viewer's active item changes (`Some(id)`) or closes (`None`).
pub type OnChange = Rc<dyn Fn(Option<&str>, &mut Window, &mut App)>;

/// Request to open the unified Stage viewer on the current window.
pub struct OpenRequest {
    pub items: Vec<ViewerItem>,
    pub active: usize,
    pub return_focus: Option<FocusHandle>,
    pub on_change: Option<OnChange>,
}

/// Global map from window id to that window's mounted [`ImageViewer`] entity.
#[derive(Default)]
struct ViewerRegistry {
    by_window: HashMap<WindowId, WeakEntity<ImageViewer>>,
}

impl Global for ViewerRegistry {}

/// Registers `viewer` as the active unified viewer for `window`. Called once by `Workspace::new`.
pub(crate) fn register(window: &Window, viewer: &Entity<ImageViewer>, cx: &mut App) {
    let id = window.window_handle().window_id();
    let registry = cx.default_global::<ViewerRegistry>();
    registry.by_window.retain(|_, weak| weak.upgrade().is_some());
    registry.by_window.insert(id, viewer.downgrade());
}

fn viewer_for(window: &Window, cx: &App) -> Option<Entity<ImageViewer>> {
    let id = window.window_handle().window_id();
    cx.try_global::<ViewerRegistry>()
        .and_then(|registry| registry.by_window.get(&id))
        .and_then(WeakEntity::upgrade)
}

/// Opens the unified Stage image viewer for `window`. Returns `true` if a viewer was registered.
pub fn open(request: OpenRequest, window: &mut Window, cx: &mut App) -> bool {
    let Some(viewer) = viewer_for(window, cx) else {
        return false;
    };
    viewer.update(cx, |viewer, cx| viewer.open_request(request, window, cx));
    true
}

/// Closes the unified Stage image viewer for `window`, if open.
pub fn close(window: &mut Window, cx: &mut App) {
    if let Some(viewer) = viewer_for(window, cx) {
        viewer.update(cx, |viewer, cx| viewer.close(window, cx));
    }
}

/// Whether the unified Stage image viewer is currently open on `window`.
pub fn is_open(window: &Window, cx: &App) -> bool {
    viewer_for(window, cx).is_some_and(|viewer| viewer.read(cx).is_open())
}

/// Zoom mode of the active image in Stage.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zoom {
    /// Scaled down to fit the stage canvas (never upscaled past 100%).
    Fit,
    /// Explicit zoom multiplier relative to natural pixel size (`1.0` = 100%, `4.0` = 400%).
    Scale(f32),
}

struct Session {
    generation: u64,
    items: Vec<ViewerItem>,
    active: usize,
    zoom: Zoom,
    return_focus: Option<FocusHandle>,
    on_change: Option<OnChange>,
    loading: HashSet<usize>,
    copied: bool,
    copied_task: Option<Task<()>>,
    save_error: Option<String>,
    tasks: Vec<Task<()>>,
}

/// Root-mounted Stage image viewer entity owned by [`crate::Workspace`].
pub struct ImageViewer {
    live: bool,
    focus: FocusHandle,
    generation: u64,
    session: Option<Session>,
}

impl ImageViewer {
    pub fn new(live: bool, cx: &mut Context<Self>) -> Self {
        Self {
            live,
            focus: cx.focus_handle(),
            generation: 0,
            session: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    pub fn active_id(&self) -> Option<&str> {
        let session = self.session.as_ref()?;
        session.items.get(session.active).map(|item| item.id.as_str())
    }

    pub fn open_request(&mut self, request: OpenRequest, window: &mut Window, cx: &mut Context<Self>) {
        if request.items.is_empty() {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let active = request.active.min(request.items.len() - 1);
        let return_focus = request.return_focus.or_else(|| window.focused(cx));
        let zoom = default_zoom(&request.items[active].read);

        self.session = Some(Session {
            generation: self.generation,
            items: request.items,
            active,
            zoom,
            return_focus,
            on_change: request.on_change,
            loading: HashSet::new(),
            copied: false,
            copied_task: None,
            save_error: None,
            tasks: Vec::new(),
        });
        window.focus(&self.focus, cx);
        self.ensure_loaded(active, cx);
        self.prefetch_filmstrip(cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.take() else {
            return;
        };
        if let Some(focus) = session.return_focus {
            window.focus(&focus, cx);
        }
        if let Some(on_change) = session.on_change {
            on_change(None, window, cx);
        }
        cx.notify();
    }

    pub fn step(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let next = session.active as isize + by;
        if let Ok(index) = usize::try_from(next)
            && index < session.items.len()
        {
            self.select(index, window, cx);
        }
    }

    pub fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (on_change, active_id) = {
            let Some(session) = &mut self.session else {
                return;
            };
            if index >= session.items.len() || index == session.active {
                return;
            }
            session.active = index;
            session.zoom = default_zoom(&session.items[index].read);
            session.copied = false;
            session.copied_task = None;
            session.save_error = None;
            (session.on_change.clone(), session.items[index].id.clone())
        };
        self.ensure_loaded(index, cx);
        self.prefetch_filmstrip(cx);
        if let Some(on_change) = on_change {
            on_change(Some(&active_id), window, cx);
        }
        cx.notify();
    }

    /// Updates a specific item's read state (for retry or external sync).
    fn retry_active(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let index = session.active;
        if session.items.get(index).is_some_and(|item| item.loader.is_some()) {
            session.items[index].read = Read::Reading;
            session.loading.remove(&index);
            self.ensure_loaded(index, cx);
            cx.notify();
        }
    }

    fn ensure_loaded(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(item) = session.items.get(index) else {
            return;
        };
        if !matches!(item.read, Read::Reading) || session.loading.contains(&index) {
            return;
        }
        let Some(loader) = item.loader.clone() else {
            return;
        };
        if !self.live {
            let result = loader();
            if let Some(item) = session.items.get_mut(index) {
                item.read = result;
            }
            if index == session.active {
                session.zoom = default_zoom(&session.items[index].read);
            }
            return;
        }
        session.loading.insert(index);
        let generation = session.generation;
        let task = cx.background_executor().spawn(async move { loader() });
        session.tasks.retain(|t| !t.is_ready());
        session.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(session) = this.session.as_mut().filter(|s| s.generation == generation) else {
                    return;
                };
                session.loading.remove(&index);
                if let Some(item) = session.items.get_mut(index) {
                    item.read = result;
                }
                if index == session.active {
                    session.zoom = default_zoom(&session.items[index].read);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn prefetch_filmstrip(&mut self, cx: &mut Context<Self>) {
        let range = {
            let Some(session) = &self.session else {
                return;
            };
            if session.items.len() <= 1 {
                return;
            }
            filmstrip_range(session.active, session.items.len())
        };
        for index in range {
            self.ensure_loaded(index, cx);
        }
    }

    fn zoom_in(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let current = effective_scale(session, window);
        let next = ZOOM_STEPS
            .iter()
            .copied()
            .find(|&step| step > current + 0.02)
            .unwrap_or(*ZOOM_STEPS.last().unwrap());
        session.zoom = Zoom::Scale(next);
        cx.notify();
    }

    fn zoom_out(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let current = effective_scale(session, window);
        let next = ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&step| step < current - 0.02)
            .unwrap_or(ZOOM_STEPS[0]);
        session.zoom = Zoom::Scale(next);
        cx.notify();
    }

    fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let item = &session.items[session.active];
        if let Read::Shown { width, height, .. } = &item.read {
            if is_tiny(*width, *height) {
                let magnified = tiny_default_scale(*width, *height);
                session.zoom = match session.zoom {
                    Zoom::Scale(s) if (s - 1.0).abs() < 0.05 => Zoom::Scale(magnified),
                    _ => Zoom::Scale(1.0),
                };
            } else {
                session.zoom = match session.zoom {
                    Zoom::Fit => Zoom::Scale(1.0),
                    Zoom::Scale(_) => Zoom::Fit,
                };
            }
            cx.notify();
        }
    }

    fn copy_active(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(image) = active_image(session) else {
            return;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::from((*image).clone()));
        session.copied = true;
        let generation = session.generation;
        let active = session.active;
        session.copied_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1400)).await;
            this.update(cx, |this, cx| {
                if let Some(session) = this.session.as_mut()
                    && session.generation == generation
                    && session.active == active
                {
                    session.copied = false;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(image) = active_image(session) else {
            return;
        };
        let name = session.items[session.active].name.clone();
        let bytes = image.bytes().to_vec();
        let dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| {
                let downloads = home.join("Downloads");
                if downloads.is_dir() { downloads } else { home }
            })
            .unwrap_or_else(std::env::temp_dir);
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        let generation = session.generation;
        session.save_error = None;
        session.tasks.retain(|t| !t.is_ready());
        session.tasks.push(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(dest))) = picked.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::write(&dest, &bytes) })
                .await;
            if let Err(error) = result {
                this.update(cx, |this, cx| {
                    if let Some(session) = this.session.as_mut().filter(|s| s.generation == generation) {
                        session.save_error = Some(format!("Couldn\u{2019}t save image: {error}"));
                        cx.notify();
                    }
                })
                .ok();
            }
        }));
    }

    fn open_active_external(&self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if let Some(path) = &session.items[session.active].local_path {
            crate::yours::editors::open(path, None, cx);
        }
    }
}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = &self.session else {
            return div().into_any_element();
        };
        let item = &session.items[session.active];
        let total = session.items.len();
        let multi = total > 1;
        let can_prev = session.active > 0;
        let can_next = session.active + 1 < total;

        let theme = cx.theme();
        let dark = theme.appearance.is_dark();
        let accent = theme.colors.accent;
        let scrim = if dark { gpui::rgba(0x0B0C0EE8) } else { gpui::rgba(0x121318DE) };
        let pill_bg = gpui::rgba(0x1E2026EB);
        let pill_edge = gpui::white().opacity(0.14);
        let fg = gpui::white().opacity(0.94);
        let fg_muted = gpui::white().opacity(0.66);
        let fg_faint = gpui::white().opacity(0.45);

        let viewport = window.viewport_size();
        let (stage_w, stage_h) = stage_bounds(window, multi);
        let has_image = matches!(item.read, Read::Shown { .. });
        let dims_meta = match &item.read {
            Read::Shown { width, height, .. } => {
                let dims = format!("{width} \u{00d7} {height}");
                let meta = match item.byte_size() {
                    Some(bytes) => format!("{dims} \u{00b7} {}", format_bytes(bytes)),
                    None => dims,
                };
                Some((meta, format!("picture-dimensions-{width}x{height}")))
            }
            _ => None,
        };

        // Top floating identity & action pills, starting past the window's own buttons at its top
        // left, as the title bar does (in full screen there are none).
        let lights = if window.is_fullscreen() { space::S5 } else { px(crate::title_bar::LIGHTS) };
        let top_bar = div()
            .flex_none()
            .w_full()
            .pl(lights)
            .pr(space::S5)
            .pt(space::S5)
            .flex()
            .items_center()
            .justify_between()
            .gap(space::S3)
            // Left floating identity pill
            .child(
                div()
                    .id("picture-identity-pill")
                    .debug_selector(|| "picture-identity-pill".into())
                    .min_w_0()
                    .max_w(px(640.))
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .px(space::S3)
                    .h(px(34.))
                    .rounded(radius::PILL)
                    .bg(pill_bg)
                    .border_1()
                    .border_color(pill_edge)
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex_none()
                            .px(px(7.))
                            .py(px(1.))
                            .rounded(radius::PILL)
                            .bg(gpui::white().opacity(0.10))
                            .text_ui(TypeSize::Caption)
                            .text_color(fg_muted)
                            .child(item.source_tag.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ui(TypeSize::Small)
                            .medium()
                            .text_color(fg)
                            .child(item.name.clone()),
                    )
                    .when_some(dims_meta, |d, (meta, selector)| {
                        d.child(
                            div()
                                .debug_selector(move || selector.clone())
                                .flex_none()
                                .text_ui(TypeSize::Caption)
                                .text_color(fg_faint)
                                .child(meta),
                        )
                    }),
            )
            // Right floating action pill + prominent Close pill
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .when(has_image, |d| {
                        let copied = session.copied;
                        let has_local = item.local_path.is_some();
                        d.child(
                            div()
                                .id("picture-actions-pill")
                                .h(px(34.))
                                .px(space::S1_5)
                                .rounded(radius::PILL)
                                .bg(pill_bg)
                                .border_1()
                                .border_color(pill_edge)
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(stage_action_btn(
                                    "picture-copy",
                                    if copied { IconName::Check } else { IconName::Copy },
                                    if copied { "Copied to clipboard" } else { "Copy image" },
                                    if copied { accent } else { fg_muted },
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.copy_active(cx);
                                    }),
                                ))
                                .child(stage_action_btn(
                                    "picture-save",
                                    IconName::ArrowDown,
                                    "Save copy\u{2026}",
                                    fg_muted,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.save_active(cx);
                                    }),
                                ))
                                .when(has_local, |d| {
                                    d.child(stage_action_btn(
                                        "picture-open-external",
                                        IconName::External,
                                        "Open in default app",
                                        fg_muted,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.open_active_external(cx);
                                        }),
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .id("picture-close")
                            .debug_selector(|| "picture-close".into())
                            .size(px(34.))
                            .rounded(radius::PILL)
                            .bg(pill_bg)
                            .border_1()
                            .border_color(gpui::white().opacity(0.22))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(gpui::rgba(0x2A2D36F2)))
                            .tooltip(|_, cx| Tooltip::new("Close (Esc)").build(cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(window, cx);
                            }))
                            .child(Icon::new(IconName::Close).size(IconSize::Nav).color(fg)),
                    ),
            );

        // Center stage body
        let stage_body: AnyElement = match &item.read {
            Read::Shown { image, width, height, .. } => {
                let tiny = is_tiny(*width, *height);
                let (draw_w, draw_h, scrollable) =
                    displayed_size(*width, *height, session.zoom, stage_w, stage_h);

                let frame: AnyElement = if tiny {
                    div()
                        .id("picture-stage-tiny")
                        .debug_selector(|| "picture-stage-tiny".into())
                        .relative()
                        .w(px(draw_w.max(64.) + 40.))
                        .h(px(draw_h.max(64.) + 40.))
                        .rounded(radius::CARD)
                        .overflow_hidden()
                        .border_1()
                        .border_color(gpui::white().opacity(0.22))
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            img(checkerboard_image())
                                .absolute()
                                .inset_0()
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
                        .child(
                            div()
                                .relative()
                                .w(px(draw_w))
                                .h(px(draw_h))
                                .child(img(image.clone()).size_full().object_fit(ObjectFit::Contain)),
                        )
                        .into_any_element()
                } else if scrollable {
                    div()
                        .id("picture-stage-scroll")
                        .max_w(px(stage_w))
                        .max_h(px(stage_h))
                        .rounded(radius::CARD)
                        .overflow_scroll()
                        .border_1()
                        .border_color(gpui::white().opacity(0.18))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .w(px(draw_w))
                                .h(px(draw_h))
                                .child(img(image.clone()).size_full().object_fit(ObjectFit::Contain)),
                        )
                        .into_any_element()
                } else {
                    div()
                        .id("picture-stage-frame")
                        .w(px(draw_w))
                        .h(px(draw_h))
                        .rounded(radius::CARD)
                        .overflow_hidden()
                        .border_1()
                        .border_color(gpui::white().opacity(0.14))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(img(image.clone()).size_full().object_fit(ObjectFit::Contain))
                        .into_any_element()
                };
                frame
            }
            Read::Missing => div()
                .id("picture-stage-missing")
                .px(space::S5)
                .py(space::S4)
                .rounded(radius::CARD)
                .bg(pill_bg)
                .border_1()
                .border_color(pill_edge)
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S2)
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(Icon::new(IconName::Image).size(IconSize::Card).color(fg_muted))
                .child(
                    div()
                        .text_ui(TypeSize::Body)
                        .text_color(fg)
                        .child("That file isn\u{2019}t there anymore."),
                )
                .into_any_element(),
            Read::Failed { why, again } => {
                let can_retry = *again && item.loader.is_some();
                div()
                    .id("picture-stage-failed")
                    .px(space::S5)
                    .py(space::S4)
                    .rounded(radius::CARD)
                    .bg(pill_bg)
                    .border_1()
                    .border_color(pill_edge)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S2)
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(Icon::new(IconName::Image).size(IconSize::Card).color(fg_muted))
                    .child(div().text_ui(TypeSize::Body).text_color(fg).child(why.clone()))
                    .when(can_retry, |d| {
                        d.child(
                            Button::new("picture-retry", "Try Again")
                                .size(ButtonSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.retry_active(cx);
                                })),
                        )
                    })
                    .into_any_element()
            }
            Read::Reading => div()
                .id("picture-stage-loading")
                .flex()
                .flex_col()
                .items_center()
                .gap(space::S2)
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(spinner("viewer-reading", IconSize::Card, fg))
                .child(div().text_ui(TypeSize::Caption).text_color(fg_muted).child("Loading image\u{2026}"))
                .into_any_element(),
        };

        let stage_canvas = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(space::S5)
            .py(space::S3)
            .flex()
            .items_center()
            .justify_center()
            .gap(space::S4)
            .when(multi, |d| {
                d.child(stage_nav_btn(
                    "picture-prev",
                    IconName::ChevronLeft,
                    "Previous image (\u{2190})",
                    can_prev,
                    fg,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.step(-1, window, cx);
                    }),
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(space::S2)
                    .child(stage_body)
                    .when_some(item.caption.clone(), |d, caption| {
                        d.child(
                            div()
                                .id("picture-caption")
                                .max_w(px(640.))
                                .px(space::S3)
                                .py(px(3.))
                                .rounded(radius::PILL)
                                .bg(gpui::rgba(0x14161ACC))
                                .text_ui(TypeSize::Small)
                                .text_color(fg_muted)
                                .text_center()
                                .truncate()
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(caption),
                        )
                    })
                    .when_some(session.save_error.clone(), |d, err| {
                        d.child(
                            div()
                                .px(space::S3)
                                .py(px(3.))
                                .rounded(radius::PILL)
                                .bg(pill_bg)
                                .text_ui(TypeSize::Caption)
                                .text_color(theme.colors.danger)
                                .child(err),
                        )
                    }),
            )
            .when(multi, |d| {
                d.child(stage_nav_btn(
                    "picture-next",
                    IconName::ChevronRight,
                    "Next image (\u{2192})",
                    can_next,
                    fg,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.step(1, window, cx);
                    }),
                ))
            });

        // Bottom floating dock: filmstrip pill (multi-image only) + zoom pill
        let filmstrip = multi.then(|| {
            let range = filmstrip_range(session.active, total);
            let active = session.active;
            div()
                .id("picture-filmstrip")
                .debug_selector(|| "picture-filmstrip".into())
                .px(space::S2)
                .py(space::S1_5)
                .rounded(radius::POPOVER)
                .bg(pill_bg)
                .border_1()
                .border_color(pill_edge)
                .flex()
                .items_center()
                .gap(space::S1_5)
                .on_click(|_, _, cx| cx.stop_propagation())
                .children(range.map(|index| {
                    let entry = &session.items[index];
                    let selected = index == active;
                    let thumb_el: AnyElement = match &entry.read {
                        Read::Shown { thumb, .. } => {
                            img(thumb.clone()).size_full().object_fit(ObjectFit::Cover).into_any_element()
                        }
                        _ => div()
                            .size_full()
                            .bg(gpui::white().opacity(0.08))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Image).size(IconSize::XSmall).color(fg_faint))
                            .into_any_element(),
                    };
                    let title = entry.name.clone();
                    div()
                        .id(("picture-thumb", index))
                        .debug_selector(move || format!("picture-thumb-{index}"))
                        .w(px(50.))
                        .h(px(34.))
                        .rounded(radius::SMALL)
                        .overflow_hidden()
                        .border_2()
                        .border_color(if selected { accent } else { gpui::white().opacity(0.14) })
                        .opacity(if selected { 1.0 } else { 0.62 })
                        .cursor_pointer()
                        .hover(|s| s.opacity(1.0))
                        .tooltip(move |_, cx| Tooltip::new(title.clone()).build(cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.select(index, window, cx);
                        }))
                        .child(thumb_el)
                }))
                .child(
                    div()
                        .debug_selector(|| "picture-counter".into())
                        .pl(space::S1_5)
                        .pr(space::S1)
                        .text_mono(TypeSize::Caption)
                        .text_color(fg_muted)
                        .child(format!("{} / {}", active + 1, total)),
                )
        });

        let zoom_pill = match &item.read {
            Read::Shown { width, height, .. } => {
                let tiny = is_tiny(*width, *height);
                let scale = effective_scale(session, window);
                let pct = (scale * 100.).round() as u32;
                let zoom_label = match session.zoom {
                    Zoom::Fit => format!("Fit \u{00b7} {pct}%"),
                    Zoom::Scale(_) => format!("{pct}%"),
                };
                let toggle_label = if tiny {
                    match session.zoom {
                        Zoom::Scale(s) if (s - 1.0).abs() < 0.05 => {
                            format!("{}%", (tiny_default_scale(*width, *height) * 100.).round() as u32)
                        }
                        _ => "1:1".into(),
                    }
                } else {
                    match session.zoom {
                        Zoom::Fit => "100%".into(),
                        Zoom::Scale(_) => "Fit".into(),
                    }
                };
                let is_custom = matches!(session.zoom, Zoom::Scale(_));
                Some(
                    div()
                        .id("picture-zoom-pill")
                        .h(px(34.))
                        .px(space::S2)
                        .rounded(radius::PILL)
                        .bg(pill_bg)
                        .border_1()
                        .border_color(pill_edge)
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(stage_action_btn(
                            "picture-zoom-out",
                            IconName::Minus,
                            "Zoom out (-)",
                            fg_muted,
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.zoom_out(window, cx);
                            }),
                        ))
                        .child(
                            div()
                                .debug_selector(|| "picture-zoom-label".into())
                                .min_w(px(58.))
                                .text_center()
                                .text_mono(TypeSize::Caption)
                                .text_color(fg)
                                .child(zoom_label),
                        )
                        .child(stage_action_btn(
                            "picture-zoom-in",
                            IconName::Plus,
                            "Zoom in (+)",
                            fg_muted,
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.zoom_in(window, cx);
                            }),
                        ))
                        .child(div().w(px(1.)).h(px(14.)).bg(pill_edge))
                        .child(
                            div()
                                .id("picture-zoom-toggle")
                                .debug_selector(|| "picture-zoom-toggle".into())
                                .px(space::S1_5)
                                .py(px(2.))
                                .rounded(radius::SMALL)
                                .when(is_custom, |d| d.bg(gpui::white().opacity(0.14)))
                                .cursor_pointer()
                                .hover(|s| s.bg(gpui::white().opacity(0.20)))
                                .text_ui(TypeSize::Caption)
                                .text_color(if is_custom { fg } else { fg_muted })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_zoom(cx);
                                }))
                                .child(toggle_label),
                        ),
                )
            }
            _ => None,
        };

        let bottom_dock = div()
            .flex_none()
            .w_full()
            .px(space::S5)
            .pb(space::S5)
            .flex()
            .items_center()
            .justify_center()
            .gap(space::S3)
            .children(filmstrip)
            .children(zoom_pill);

        let overlay = div()
            .id("picture-viewer-layer")
            .debug_selector(|| "picture-viewer".into())
            .w(viewport.width)
            .h(viewport.height)
            .occlude()
            .track_focus(&self.focus)
            .bg(scrim)
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.close(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.close(window, cx);
                        cx.stop_propagation();
                    }
                    "left" => {
                        this.step(-1, window, cx);
                        cx.stop_propagation();
                    }
                    "right" => {
                        this.step(1, window, cx);
                        cx.stop_propagation();
                    }
                    "+" | "=" => {
                        this.zoom_in(window, cx);
                        cx.stop_propagation();
                    }
                    "-" => {
                        this.zoom_out(window, cx);
                        cx.stop_propagation();
                    }
                    "0" => {
                        this.toggle_zoom(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(top_bar)
            .child(stage_canvas)
            .child(bottom_dock);

        deferred(anchored().position(point(px(0.), px(0.))).child(overlay))
            .with_priority(10)
            .into_any_element()
    }
}

fn active_image(session: &Session) -> Option<Arc<Image>> {
    match &session.items.get(session.active)?.read {
        Read::Shown { image, .. } => Some(image.clone()),
        _ => None,
    }
}

fn is_tiny(width: u32, height: u32) -> bool {
    width.max(height) <= TINY_SIDE
}

fn tiny_default_scale(width: u32, height: u32) -> f32 {
    if width.max(height) <= 48 { 4.0 } else { 2.0 }
}

fn default_zoom(read: &Read) -> Zoom {
    match read {
        Read::Shown { width, height, .. } if is_tiny(*width, *height) => {
            Zoom::Scale(tiny_default_scale(*width, *height))
        }
        _ => Zoom::Fit,
    }
}

fn stage_bounds(window: &Window, multi: bool) -> (f32, f32) {
    let size = window.viewport_size();
    let horizontal_chrome = if multi { 184. } else { 96. };
    let vertical_chrome = 188.;
    (
        (f32::from(size.width) - horizontal_chrome).max(160.),
        (f32::from(size.height) - vertical_chrome).max(140.),
    )
}

fn effective_scale(session: &Session, window: &Window) -> f32 {
    match session.zoom {
        Zoom::Scale(s) => s,
        Zoom::Fit => match &session.items[session.active].read {
            Read::Shown { width, height, .. } => {
                let (max_w, max_h) = stage_bounds(window, session.items.len() > 1);
                let (w, h) = pictures::fitted(*width, *height, max_w, max_h);
                (w / (*width).max(1) as f32).min(h / (*height).max(1) as f32)
            }
            _ => 1.0,
        },
    }
}

fn displayed_size(width: u32, height: u32, zoom: Zoom, max_w: f32, max_h: f32) -> (f32, f32, bool) {
    match zoom {
        Zoom::Fit => {
            let (w, h) = pictures::fitted(width, height, max_w, max_h);
            (w, h, false)
        }
        Zoom::Scale(scale) => {
            let w = (width as f32 * scale).max(1.);
            let h = (height as f32 * scale).max(1.);
            let scrollable = w > max_w + 1. || h > max_h + 1.;
            (w, h, scrollable)
        }
    }
}

fn filmstrip_range(active: usize, total: usize) -> std::ops::Range<usize> {
    if total <= FILMSTRIP_WINDOW {
        return 0..total;
    }
    let half = FILMSTRIP_WINDOW / 2;
    let start = active.saturating_sub(half).min(total - FILMSTRIP_WINDOW);
    start..(start + FILMSTRIP_WINDOW)
}

fn stage_action_btn(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    color: Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .size(px(26.))
        .rounded(radius::PILL)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(gpui::white().opacity(0.14)))
        .tooltip(move |_, cx| Tooltip::new(tooltip).build(cx))
        .on_click(on_click)
        .child(Icon::new(icon).size(IconSize::Nav).color(color))
        .into_any_element()
}

fn stage_nav_btn(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    enabled: bool,
    fg: Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .size(px(40.))
        .rounded(radius::PILL)
        .bg(gpui::rgba(0x1E2026EB))
        .border_1()
        .border_color(gpui::white().opacity(if enabled { 0.16 } else { 0.08 }))
        .opacity(if enabled { 1.0 } else { 0.35 })
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(gpui::rgba(0x2D303AF2)))
                .tooltip(move |_, cx| Tooltip::new(tooltip).build(cx))
                .on_click(on_click)
        })
        .when(!enabled, |d| d.on_click(|_, _, cx| cx.stop_propagation()))
        .child(Icon::new(icon).size(IconSize::Medium).color(fg))
        .into_any_element()
}

fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", (b / KB).max(1.0))
    } else {
        format!("{bytes} B")
    }
}

fn checkerboard_image() -> Arc<Image> {
    static CHECKER: OnceLock<Arc<Image>> = OnceLock::new();
    CHECKER
        .get_or_init(|| {
            let img = image::RgbaImage::from_fn(256, 256, |x, y| {
                let light = ((x / 12) + (y / 12)) % 2 == 0;
                if light {
                    image::Rgba([56, 58, 66, 255])
                } else {
                    image::Rgba([38, 40, 46, 255])
                }
            });
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).expect("checkerboard PNG");
            Arc::new(Image::from_bytes(ImageFormat::Png, out.into_inner()))
        })
        .clone()
}

/// Converts a file path on disk into a `file://` URI suitable for `kuvo_box::pictures::Source::File`.
pub(crate) fn file_uri(path: &str) -> String {
    if path.starts_with("file://") {
        path.to_owned()
    } else {
        kuvo_box::pictures::file_uri(path)
    }
}

/// Whether `path` looks like an image file supported by the Stage viewer.
pub(crate) fn is_image_path(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" | "ico")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_escapes_filename_and_preserves_existing_addresses() {
        let uri = "file:///tmp/scene%20%231%3F%20100%25%20%C3%BC.png";
        assert_eq!(file_uri("/tmp/scene #1? 100% ü.png"), uri);
        assert_eq!(file_uri(uri), uri);
    }

    #[test]
    fn filmstrip_window_centers_around_active_index() {
        assert_eq!(filmstrip_range(0, 3), 0..3);
        assert_eq!(filmstrip_range(0, 12), 0..7);
        assert_eq!(filmstrip_range(5, 12), 2..9);
        assert_eq!(filmstrip_range(11, 12), 5..12);
    }

    #[test]
    fn tiny_images_default_to_magnified_zoom_and_large_images_fit() {
        let tiny = Read::Shown {
            image: checkerboard_image(),
            thumb: checkerboard_image(),
            width: 32,
            height: 32,
        };
        assert_eq!(default_zoom(&tiny), Zoom::Scale(4.0));

        let medium_small = Read::Shown {
            image: checkerboard_image(),
            thumb: checkerboard_image(),
            width: 80,
            height: 80,
        };
        assert_eq!(default_zoom(&medium_small), Zoom::Scale(2.0));

        let normal = Read::Shown {
            image: checkerboard_image(),
            thumb: checkerboard_image(),
            width: 1440,
            height: 900,
        };
        assert_eq!(default_zoom(&normal), Zoom::Fit);
    }
}
