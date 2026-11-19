//! Shared Kuvo desktop window shell and rich synthetic sample images for stand-in design drafts.
//! Never compiled into shipped builds (`#[cfg(feature = "stand-in")]` only).

use std::sync::Arc;

use gpui::{
    AnyElement, App, Hsla, Image, ImageFormat, IntoElement, ObjectFit, ParentElement, Pixels,
    SharedString, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px,
};
use kuvo_ui::{
    ActiveTheme, Icon, IconName, IconSize, StyledType, TextRole, TypeSize,
    small::status_dot,
    theme::{radius, space},
};

/// A synthetic image asset used by both the image viewer drafts and the Outputs popover drafts.
#[derive(Clone)]
pub struct SampleImage {
    pub id: &'static str,
    pub name: &'static str,
    pub short_label: &'static str,
    pub caption: Option<&'static str>,
    pub source_tag: &'static str,
    pub format_label: &'static str,
    pub size_label: &'static str,
    /// Reported natural pixel dimensions (may exceed raster size for simulated 5K/tall captures).
    pub natural_w: u32,
    pub natural_h: u32,
    pub transparent: bool,
    pub image: Arc<Image>,
}

/// Complete gallery of synthetic sample images: UI screenshots, charts, a huge full-page capture,
/// and a tiny 32x32 transparent badge icon.
pub struct SampleGallery {
    pub multi: Vec<SampleImage>,
    pub single: SampleImage,
    pub very_large: SampleImage,
    pub very_small: SampleImage,
}

impl SampleGallery {
    pub fn build() -> Self {
        let img_insights_light = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(false, [10, 122, 255], [46, 158, 91], MockupKind::Analytics),
        ));
        let img_insights_dark = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(true, [79, 156, 245], [76, 194, 122], MockupKind::Analytics),
        ));
        let img_popover_light = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(false, [120, 92, 246], [242, 115, 92], MockupKind::Cards),
        ));
        let img_pairing_dark = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(true, [94, 199, 184], [245, 166, 61], MockupKind::Timeline),
        ));
        let img_usage_light = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(false, [235, 94, 72], [10, 122, 255], MockupKind::Rings),
        ));
        let img_sidebar_dark = Arc::new(Image::from_bytes(
            ImageFormat::Png,
            render_ui_mockup(true, [168, 122, 255], [76, 194, 122], MockupKind::Cards),
        ));

        let multi = vec![
            SampleImage {
                id: "img_1",
                name: "screens-insights-overview-light.png",
                short_label: "insights-light",
                caption: Some("Usage overview in light mode — 7-day token trend and per-provider quota rings"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "318 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_insights_light,
            },
            SampleImage {
                id: "img_2",
                name: "screens-insights-overview-dark.png",
                short_label: "insights-dark",
                caption: Some("Usage overview in dark mode — verified contrast on sunken chart cards"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "294 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_insights_dark,
            },
            SampleImage {
                id: "img_3",
                name: "screens-outputs-popover-light.png",
                short_label: "popover-light",
                caption: Some("Outputs popover after grouping subagents and filtering each output kind"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "412 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_popover_light,
            },
            SampleImage {
                id: "img_4",
                name: "screens-computer-pairing-dark.png",
                short_label: "pairing-dark",
                caption: Some("Pairing timeline with live handover states on remote Mac mini"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "276 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_pairing_dark,
            },
            SampleImage {
                id: "img_5",
                name: "screens-usage-rings-light.png",
                short_label: "rings-light",
                caption: Some("Provider usage rings at 5h and 7d cadence with reset countdown"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "248 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_usage_light,
            },
            SampleImage {
                id: "img_6",
                name: "screens-bots-sidebar-dark.png",
                short_label: "bots-dark",
                caption: Some("Bots sidebar states — Needs You, Working, Scheduled, and Offline"),
                source_tag: "Outputs",
                format_label: "PNG",
                size_label: "305 KB",
                natural_w: 2560,
                natural_h: 1640,
                transparent: false,
                image: img_sidebar_dark,
            },
        ];

        let single = SampleImage {
            id: "img_single",
            name: "checkout-validation-fix.png",
            short_label: "checkout-fix",
            caption: Some("Inline email validation keeps focus on the email field after a 409 conflict"),
            source_tag: "Conversation",
            format_label: "PNG",
            size_label: "284 KB",
            natural_w: 1920,
            natural_h: 1200,
            transparent: false,
            image: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                render_ui_mockup(false, [10, 122, 255], [46, 158, 91], MockupKind::Analytics),
            )),
        };

        let very_large = SampleImage {
            id: "img_large",
            name: "fullpage-release-audit-5k.png",
            short_label: "audit-5k",
            caption: Some("Full-page audit capture — zoomed to 100% (drag or scroll to pan)"),
            source_tag: "Side Panel",
            format_label: "PNG",
            size_label: "6.4 MB",
            natural_w: 3840,
            natural_h: 5200,
            transparent: false,
            image: Arc::new(Image::from_bytes(ImageFormat::Png, render_large_capture())),
        };

        let very_small = SampleImage {
            id: "img_small",
            name: "kuvo-tray-badge@1x.png",
            short_label: "tray-badge",
            caption: Some("Menu bar glyph asset — transparent PNG shown on checkerboard matte"),
            source_tag: "Attachment",
            format_label: "PNG · Alpha",
            size_label: "1.2 KB",
            natural_w: 32,
            natural_h: 32,
            transparent: true,
            image: Arc::new(Image::from_bytes(ImageFormat::Png, render_tiny_icon())),
        };

        Self { multi, single, very_large, very_small }
    }
}

#[derive(Clone, Copy)]
enum MockupKind {
    Analytics,
    Cards,
    Timeline,
    Rings,
}

/// Renders a crisp, realistic miniature app UI mockup (760x480) so viewer and popover thumbnails
/// look like real screenshots rather than flat placeholder rectangles.
fn render_ui_mockup(dark: bool, accent: [u8; 3], accent2: [u8; 3], kind: MockupKind) -> Vec<u8> {
    let (w, h) = (760u32, 480u32);
    let bg = if dark { [16u8, 17, 20] } else { [243u8, 244, 247] };
    let side = if dark { [23u8, 24, 28] } else { [249u8, 249, 251] };
    let card = if dark { [32u8, 34, 40] } else { [255u8, 255, 255] };
    let border = if dark { [48u8, 51, 59] } else { [224u8, 226, 232] };
    let text_hi = if dark { [226u8, 229, 236] } else { [45u8, 48, 56] };
    let text_lo = if dark { [88u8, 94, 108] } else { [176u8, 181, 192] };

    let mut buf = image::RgbImage::from_pixel(w, h, image::Rgb(bg));

    let fill_rect = |img: &mut image::RgbImage, x0: u32, y0: u32, rw: u32, rh: u32, col: [u8; 3]| {
        for y in y0..(y0 + rh).min(h) {
            for x in x0..(x0 + rw).min(w) {
                img.put_pixel(x, y, image::Rgb(col));
            }
        }
    };
    let stroke_rect = |img: &mut image::RgbImage, x0: u32, y0: u32, rw: u32, rh: u32, col: [u8; 3]| {
        fill_rect(img, x0, y0, rw, 1, col);
        fill_rect(img, x0, y0 + rh.saturating_sub(1), rw, 1, col);
        fill_rect(img, x0, y0, 1, rh, col);
        fill_rect(img, x0 + rw.saturating_sub(1), y0, 1, rh, col);
    };
    let fill_circle = |img: &mut image::RgbImage, cx: i32, cy: i32, r: i32, col: [u8; 3]| {
        for y in (cy - r).max(0)..=(cy + r).min(h as i32 - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(w as i32 - 1) {
                let dx = x - cx;
                let dy = y - cy;
                if dx * dx + dy * dy <= r * r {
                    img.put_pixel(x as u32, y as u32, image::Rgb(col));
                }
            }
        }
    };
    let draw_ring = |img: &mut image::RgbImage, cx: i32, cy: i32, r_out: i32, r_in: i32, col: [u8; 3], track: [u8; 3], frac: f32| {
        for y in (cy - r_out).max(0)..=(cy + r_out).min(h as i32 - 1) {
            for x in (cx - r_out).max(0)..=(cx + r_out).min(w as i32 - 1) {
                let dx = x - cx;
                let dy = y - cy;
                let d2 = dx * dx + dy * dy;
                if d2 <= r_out * r_out && d2 >= r_in * r_in {
                    let angle = (dy as f32).atan2(dx as f32) + std::f32::consts::FRAC_PI_2;
                    let norm = (angle.rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU;
                    let c = if norm <= frac { col } else { track };
                    img.put_pixel(x as u32, y as u32, image::Rgb(c));
                }
            }
        }
    };

    // Title bar & sidebar
    fill_rect(&mut buf, 0, 0, w, 34, side);
    fill_rect(&mut buf, 0, 34, 152, h - 34, side);
    fill_rect(&mut buf, 0, 34, w, 1, border);
    fill_rect(&mut buf, 152, 34, 1, h - 34, border);

    // Traffic lights
    fill_circle(&mut buf, 18, 17, 5, [238, 106, 95]);
    fill_circle(&mut buf, 34, 17, 5, [244, 191, 79]);
    fill_circle(&mut buf, 50, 17, 5, [97, 197, 84]);
    // Header title bar pill
    fill_rect(&mut buf, 172, 12, 130, 10, text_lo);
    fill_rect(&mut buf, w - 76, 10, 56, 14, accent);

    // Sidebar items
    for i in 0..7u32 {
        let y = 52 + i * 32;
        if i == 1 {
            fill_rect(&mut buf, 10, y - 5, 132, 24, card);
            stroke_rect(&mut buf, 10, y - 5, 132, 24, border);
            fill_circle(&mut buf, 22, (y + 7) as i32, 4, accent);
            fill_rect(&mut buf, 34, y + 3, 86, 8, text_hi);
        } else {
            fill_circle(&mut buf, 22, (y + 7) as i32, 4, text_lo);
            fill_rect(&mut buf, 34, y + 3, 70 + (i % 3) * 14, 7, text_lo);
        }
    }

    // Main content header
    fill_rect(&mut buf, 176, 54, 170, 14, text_hi);
    fill_rect(&mut buf, 176, 76, 260, 8, text_lo);

    match kind {
        MockupKind::Analytics => {
            // 3 KPI cards
            for i in 0..3u32 {
                let x = 176 + i * 190;
                fill_rect(&mut buf, x, 102, 176, 76, card);
                stroke_rect(&mut buf, x, 102, 176, 76, border);
                fill_rect(&mut buf, x + 14, 116, 68, 8, text_lo);
                fill_rect(&mut buf, x + 14, 134, 96, 16, if i == 0 { accent } else { text_hi });
                fill_rect(&mut buf, x + 14, 158, 118, 6, if i == 1 { accent2 } else { text_lo });
            }
            // Bar chart card
            fill_rect(&mut buf, 176, 194, 556, 260, card);
            stroke_rect(&mut buf, 176, 194, 556, 260, border);
            fill_rect(&mut buf, 196, 212, 140, 10, text_hi);
            for gy in 0..4u32 {
                fill_rect(&mut buf, 196, 244 + gy * 46, 516, 1, border);
            }
            let heights = [92u32, 134, 112, 164, 148, 178, 126, 158, 104, 142, 170, 122];
            for (idx, bh) in heights.iter().enumerate() {
                let bx = 208 + idx as u32 * 42;
                let by = 428 - *bh;
                let col = if idx == 5 || idx == 10 { accent } else { accent2 };
                fill_rect(&mut buf, bx, by, 24, *bh, col);
            }
        }
        MockupKind::Cards => {
            for row in 0..2u32 {
                for col in 0..2u32 {
                    let x = 176 + col * 284;
                    let y = 104 + row * 176;
                    fill_rect(&mut buf, x, y, 270, 160, card);
                    stroke_rect(&mut buf, x, y, 270, 160, border);
                    fill_rect(&mut buf, x + 16, y + 18, 110, 10, text_hi);
                    fill_rect(&mut buf, x + 16, y + 36, 180, 7, text_lo);
                    let bar_col = if (row + col) % 2 == 0 { accent } else { accent2 };
                    fill_rect(&mut buf, x + 16, y + 60, 238, 54, bg);
                    fill_rect(&mut buf, x + 28, y + 74, 140, 10, bar_col);
                    fill_rect(&mut buf, x + 28, y + 92, 190, 7, text_lo);
                    fill_rect(&mut buf, x + 16, y + 130, 82, 14, bar_col);
                }
            }
        }
        MockupKind::Timeline => {
            fill_rect(&mut buf, 176, 104, 556, 350, card);
            stroke_rect(&mut buf, 176, 104, 556, 350, border);
            fill_rect(&mut buf, 214, 128, 2, 300, border);
            for i in 0..5u32 {
                let y = 136 + i * 60;
                let dot = if i < 2 { accent } else if i == 2 { accent2 } else { text_lo };
                fill_circle(&mut buf, 215, (y + 12) as i32, 7, dot);
                fill_rect(&mut buf, 240, y + 4, 190 + (i % 3) * 55, 10, text_hi);
                fill_rect(&mut buf, 240, y + 22, 300 + (i % 2) * 80, 7, text_lo);
                fill_rect(&mut buf, 650, y + 8, 58, 14, bg);
            }
        }
        MockupKind::Rings => {
            for i in 0..3u32 {
                let x = 176 + i * 190;
                fill_rect(&mut buf, x, 104, 176, 200, card);
                stroke_rect(&mut buf, x, 104, 176, 200, border);
                let col = match i {
                    0 => accent,
                    1 => accent2,
                    _ => [138, 92, 245],
                };
                let frac = match i {
                    0 => 0.72,
                    1 => 0.44,
                    _ => 0.86,
                };
                draw_ring(&mut buf, (x + 88) as i32, 192, 46, 34, col, border, frac);
                fill_rect(&mut buf, x + 56, 187, 64, 10, text_hi);
                fill_rect(&mut buf, x + 36, 260, 104, 8, text_lo);
                fill_rect(&mut buf, x + 50, 276, 76, 7, text_lo);
            }
            fill_rect(&mut buf, 176, 322, 556, 132, card);
            stroke_rect(&mut buf, 176, 322, 556, 132, border);
            for i in 0..3u32 {
                let y = 342 + i * 34;
                fill_rect(&mut buf, 196, y, 160, 9, text_hi);
                fill_rect(&mut buf, 380, y + 1, 240, 8, bg);
                fill_rect(&mut buf, 380, y + 1, 120 + i * 44, 8, if i == 1 { accent2 } else { accent });
            }
        }
    }

    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).expect("encode mockup png");
    out.into_inner()
}

/// Renders a dense high-detail audit grid simulating a zoomed-in 100% viewport crop of a 3840x5200
/// full-page capture (sharp pixel grid, code/diff columns, annotations).
fn render_large_capture() -> Vec<u8> {
    let (w, h) = (960u32, 640u32);
    let mut buf = image::RgbImage::from_pixel(w, h, image::Rgb([24u8, 25, 30]));
    let fill = |img: &mut image::RgbImage, x0: u32, y0: u32, rw: u32, rh: u32, col: [u8; 3]| {
        for y in y0..(y0 + rh).min(h) {
            for x in x0..(x0 + rw).min(w) {
                img.put_pixel(x, y, image::Rgb(col));
            }
        }
    };

    // Subtle blueprint grid lines so 100% zoom feels tactile and crisp
    for x in (0..w).step_by(32) {
        fill(&mut buf, x, 0, 1, h, [31, 33, 40]);
    }
    for y in (0..h).step_by(32) {
        fill(&mut buf, 0, y, w, 1, [31, 33, 40]);
    }

    // Two side-by-side audit cards inside the large capture
    fill(&mut buf, 28, 24, 436, 592, [33, 36, 44]);
    fill(&mut buf, 496, 24, 436, 592, [33, 36, 44]);
    // Header accents
    fill(&mut buf, 28, 24, 436, 4, [46, 139, 255]);
    fill(&mut buf, 496, 24, 436, 4, [76, 194, 122]);

    // Left column: diff lines
    for row in 0..16u32 {
        let y = 52 + row * 34;
        if row == 4 || row == 5 {
            fill(&mut buf, 40, y - 4, 412, 26, [66, 34, 38]);
            fill(&mut buf, 48, y + 4, 16, 10, [240, 99, 99]);
            fill(&mut buf, 76, y + 5, 220 + (row % 3) * 40, 8, [245, 168, 168]);
        } else if row == 6 || row == 7 || row == 8 {
            fill(&mut buf, 40, y - 4, 412, 26, [30, 62, 46]);
            fill(&mut buf, 48, y + 4, 16, 10, [76, 194, 122]);
            fill(&mut buf, 76, y + 5, 240 + (row % 2) * 60, 8, [160, 232, 186]);
        } else {
            fill(&mut buf, 48, y + 5, 22, 8, [86, 92, 108]);
            fill(&mut buf, 84, y + 5, 180 + (row * 29) % 190, 8, [168, 174, 188]);
        }
    }

    // Right column: latency chart + trace bars
    fill(&mut buf, 520, 52, 388, 220, [24, 26, 32]);
    for i in 0..14u32 {
        let bx = 536 + i * 26;
        let bh = 40 + ((i * 37 + 19) % 145);
        let col = if bh > 140 { [240, 164, 58] } else { [46, 139, 255] };
        fill(&mut buf, bx, 256 - bh, 16, bh, col);
    }
    for row in 0..8u32 {
        let y = 300 + row * 38;
        fill(&mut buf, 520, y, 388, 26, [26, 28, 35]);
        let start = 532 + (row * 24) % 120;
        let span = 90 + (row * 41) % 160;
        let col = match row % 3 {
            0 => [46, 139, 255],
            1 => [76, 194, 122],
            _ => [168, 122, 255],
        };
        fill(&mut buf, start, y + 7, span, 12, col);
    }

    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).expect("encode large png");
    out.into_inner()
}

/// Renders a crisp 32x32 transparent PNG badge icon (rounded pebble with Kuvo eyes) so the
/// very-small-image case genuinely demonstrates alpha transparency and pixel clarity.
fn render_tiny_icon() -> Vec<u8> {
    let size = 32u32;
    let mut buf = image::RgbaImage::from_pixel(size, size, image::Rgba([0, 0, 0, 0]));
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 - 15.5;
            let fy = y as f32 - 15.5;
            // Squircle badge silhouette
            let norm = (fx.abs() / 13.0).powf(3.2) + (fy.abs() / 13.0).powf(3.2);
            if norm <= 1.0 {
                // Smooth sky-blue pebble fill with top highlight
                let t = (y as f32) / 31.0;
                let r = (36.0 + t * 14.0) as u8;
                let g = (148.0 - t * 30.0) as u8;
                let b = (255.0 - t * 20.0) as u8;
                let mut px = image::Rgba([r, g, b, 255]);
                // Outer 1px rim
                if norm > 0.80 {
                    px = image::Rgba([22, 102, 210, 255]);
                }
                // Eyes
                let left_eye = (fx + 4.2).hypot(fy + 1.5);
                let right_eye = (fx - 4.2).hypot(fy + 1.5);
                if left_eye <= 2.3 || right_eye <= 2.3 {
                    px = image::Rgba([255, 255, 255, 255]);
                }
                if (fx + 3.8).hypot(fy + 1.5) <= 1.1 || (fx - 4.6).hypot(fy + 1.5) <= 1.1 {
                    px = image::Rgba([20, 32, 54, 255]);
                }
                // Smile arc
                let smile_r = fx.hypot(fy - 2.0);
                if fy > 3.2 && fy < 6.8 && fx.abs() < 4.5 && (smile_r - 3.6).abs() < 0.9 {
                    px = image::Rgba([255, 255, 255, 255]);
                }
                buf.put_pixel(x, y, px);
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).expect("encode tiny png");
    out.into_inner()
}

/// Renders a subtle checkerboard PNG for staging transparent and very small images.
pub fn checkerboard_image(dark: bool) -> Arc<Image> {
    let (w, h) = (160u32, 160u32);
    let cell = 10u32;
    let (a, b) = if dark { ([34u8, 36, 40], [44u8, 46, 52]) } else { ([236u8, 237, 240], [248u8, 248, 250]) };
    let buf = image::RgbImage::from_fn(w, h, |x, y| {
        if ((x / cell) + (y / cell)) % 2 == 0 {
            image::Rgb(a)
        } else {
            image::Rgb(b)
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png).expect("checkerboard png");
    Arc::new(Image::from_bytes(ImageFormat::Png, out.into_inner()))
}

/// Renders a faithful Kuvo desktop window shell (title bar with Activity button, area rail,
/// Sessions sidebar, conversation flow with realistic turns, and composer), with an optional
/// full-window overlay (for viewer drafts) or anchored popover (for Outputs popover drafts).
pub fn kuvo_window_shell(
    draft_badge: &str,
    activity_open: bool,
    activity_running: bool,
    thumbs: &[&SampleImage],
    overlay: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(c.background)
        .text_ui(TypeSize::Body)
        .text_color(c.text)
        .overflow_hidden()
        .child(title_bar(draft_badge, activity_open, activity_running, cx))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(rail(cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .rounded_tl(radius::POPOVER)
                        .border_t_1()
                        .border_l_1()
                        .border_color(c.border)
                        .overflow_hidden()
                        .child(sidebar(cx))
                        .child(conversation_pane(thumbs, cx)),
                ),
        )
        .children(overlay)
        .into_any_element()
}

fn title_bar(draft_badge: &str, activity_open: bool, activity_running: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let badge = SharedString::from(draft_badge.to_owned());
    div()
        .h(px(44.))
        .flex_none()
        .flex()
        .items_center()
        .px(space::S3)
        .gap(space::S2)
        // Traffic lights
        .child(
            div()
                .w(px(64.))
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(space::S1)
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong))
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong))
                .child(div().size(px(12.)).rounded(radius::PILL).bg(c.border_strong)),
        )
        // Computer switcher + sidebar toggle
        .child(
            div()
                .w(px(218.))
                .flex()
                .items_center()
                .gap(space::S2)
                .child(Icon::new(IconName::SidebarLeft).size(IconSize::Medium).color(c.icon))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1)
                        .px(space::S2)
                        .py(space::S1)
                        .rounded(radius::NAV)
                        .text_ui(TypeSize::Small)
                        .text_color(c.text_muted)
                        .child("This Mac")
                        .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall).color(c.text_faint)),
                )
                .child(div().flex_1())
                .child(Icon::new(IconName::ArrowLeft).size(IconSize::Medium).color(c.text_faint))
                .child(Icon::new(IconName::ArrowRight).size(IconSize::Medium).color(c.text_faint)),
        )
        .child(div().w(px(1.)).h(px(16.)).bg(c.border))
        // Session title + draft tag
        .child(
            div()
                .flex_1()
                .min_w_0()
                .pl(space::S2)
                .flex()
                .items_center()
                .gap(space::S2)
                .child(div().text_role(TextRole::WindowTitle).text_color(c.text).truncate().child("Kuvo Foreman"))
                .child(div().text_role(TextRole::WindowContext).text_color(c.text_faint).child("kuvo-code · main"))
                .child(
                    div()
                        .ml(space::S1)
                        .px(space::S2)
                        .py(px(2.))
                        .rounded(radius::PILL)
                        .bg(c.sunken)
                        .border_1()
                        .border_color(c.border)
                        .text_ui(TypeSize::Caption)
                        .text_color(c.text_muted)
                        .child(badge),
                ),
        )
        // Right title bar actions: Activity trigger + Panel toggle
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::S1)
                .child(
                    div()
                        .relative()
                        .size(px(30.))
                        .rounded(radius::NAV)
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(activity_open, |d| d.bg(c.selected))
                        .child(
                            Icon::new(IconName::Activity)
                                .size(IconSize::Large)
                                .color(if activity_open { c.icon_on } else { c.icon }),
                        )
                        .when(activity_running, |d| {
                            d.child(div().absolute().top(px(4.)).right(px(4.)).child(status_dot(c.accent)))
                        }),
                )
                .child(
                    div()
                        .size(px(30.))
                        .rounded(radius::NAV)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::PanelRight).size(IconSize::Large).color(c.icon)),
                ),
        )
        .into_any_element()
}

fn rail(cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let item = |icon: IconName, active: bool| {
        div()
            .size(px(38.))
            .rounded(radius::NAV)
            .flex()
            .items_center()
            .justify_center()
            .when(active, |d| d.bg(c.selected_card).border_1().border_color(c.selected_edge))
            .child(Icon::new(icon).size(IconSize::Rail).color(if active { c.icon_on } else { c.icon }))
    };
    div()
        .w(px(56.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .py(space::S2)
        .gap(space::S1_5)
        .child(item(IconName::Message, true))
        .child(item(IconName::Bot, false))
        .child(item(IconName::Queue, false))
        .child(item(IconName::Computer, false))
        .child(div().flex_1())
        .child(item(IconName::Settings, false))
        .into_any_element()
}

fn sidebar(cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let theme = cx.theme();
    let rows = [
        ("Kuvo Foreman", "3 running · 200 outputs", true, true),
        ("Release beta.35 rehearsal", "14m ago", false, false),
        ("Insights usage ring polish", "1h ago", false, false),
        ("Fix signup focus on 409", "3h ago", false, false),
        ("Work queue Linear refs", "Yesterday", false, false),
    ];
    div()
        .w(px(236.))
        .flex_none()
        .h_full()
        .bg(c.sidebar)
        .border_r_1()
        .border_color(c.border)
        .p(space::S2)
        .flex()
        .flex_col()
        .gap(space::S1)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S2)
                .py(space::S1_5)
                .child(div().text_role(TextRole::PaneTitle).text_color(c.text).child("Sessions"))
                .child(Icon::new(IconName::Compose).size(IconSize::Medium).color(c.icon)),
        )
        .children(rows.into_iter().map(|(title, sub, active, live)| {
            div()
                .px(space::S2)
                .py(space::S2)
                .rounded(radius::NAV)
                .flex()
                .flex_col()
                .gap(px(2.))
                .when(active, |d| {
                    d.bg(c.selected_card)
                        .border_1()
                        .border_color(c.selected_edge)
                        .shadow(theme.selected_shadow())
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S1_5)
                        .when(live, |d| d.child(status_dot(c.accent)))
                        .child(div().flex_1().min_w_0().truncate().text_ui(TypeSize::Small).medium().text_color(c.text).child(title)),
                )
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).truncate().child(sub))
        }))
        .into_any_element()
}

fn conversation_pane(thumbs: &[&SampleImage], cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(c.surface)
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .justify_center()
                .px(space::S6)
                .py(space::S5)
                .overflow_hidden()
                .child(
                    div()
                        .w(px(680.))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .gap(space::S4)
                        // User turn
                        .child(
                            div().w_full().flex().justify_end().child(
                                div()
                                    .max_w(px(500.))
                                    .px(space::S3)
                                    .py(space::S2)
                                    .rounded(radius::CARD)
                                    .bg(c.sunken)
                                    .text_ui(TypeSize::Small)
                                    .text_color(c.text)
                                    .child("Show the latest beta.35 verification screenshots and keep the nightly load check running."),
                            ),
                        )
                        // Agent turn
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(space::S3)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(space::S1_5)
                                        .text_ui(TypeSize::Caption)
                                        .text_color(c.text_faint)
                                        .child(Icon::new(IconName::Check).size(IconSize::XSmall).color(c.success))
                                        .child("Ran 4 checks · 6 screenshots captured"),
                                )
                                .child(
                                    div()
                                        .text_ui(TypeSize::Body)
                                        .text_color(c.text)
                                        .child("Rendered the light and dark checks for Insights, Pairing, and the Outputs popover. The Mini load watch and beta.35 rehearsal subagents are active in the background."),
                                )
                                .when(!thumbs.is_empty(), |d| {
                                    d.child(
                                        div().flex().gap(space::S2).children(thumbs.iter().take(3).map(|sample| {
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .gap(space::S1)
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .h(px(116.))
                                                        .rounded(radius::POPOVER)
                                                        .overflow_hidden()
                                                        .bg(c.sunken)
                                                        .border_1()
                                                        .border_color(c.border)
                                                        .child(img(sample.image.clone()).size_full().object_fit(ObjectFit::Cover)),
                                                )
                                                .child(
                                                    div()
                                                        .text_ui(TypeSize::Caption)
                                                        .text_color(c.text_muted)
                                                        .truncate()
                                                        .child(sample.short_label),
                                                )
                                        })),
                                    )
                                }),
                        ),
                ),
        )
        // Quiet composer at bottom
        .child(
            div()
                .flex_none()
                .px(space::S6)
                .pb(space::S4)
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(680.))
                        .max_w_full()
                        .rounded(radius::CARD)
                        .bg(c.elevated)
                        .border_1()
                        .border_color(c.border_strong)
                        .px(space::S3)
                        .py(space::S2)
                        .flex()
                        .flex_col()
                        .gap(space::S2)
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_faint).child("Reply to Kuvo Foreman…"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(space::S2)
                                        .child(Icon::new(IconName::Paperclip).size(IconSize::Nav).color(c.icon))
                                        .child(div().text_ui(TypeSize::Caption).text_color(c.text_muted).child("Claude Opus 4.6")),
                                )
                                .child(
                                    div()
                                        .size(px(26.))
                                        .rounded(radius::PILL)
                                        .bg(c.primary)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(Icon::new(IconName::ArrowUp).size(IconSize::Nav).color(c.text_on_primary)),
                                ),
                        ),
                ),
        )
        .into_any_element()
}

/// Review board wrapper for side-by-side multi-state draft comparison sheets.
pub fn review_board(title: &str, subtitle: &str, body: AnyElement, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .size_full()
        .bg(c.background)
        .p(space::S5)
        .flex()
        .flex_col()
        .gap(space::S4)
        .text_ui(TypeSize::Small)
        .text_color(c.text)
        .child(
            div()
                .flex_none()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(space::S4)
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(space::S2)
                        .child(div().text_role(TextRole::PaneTitle).text_color(c.text).child(title.to_owned()))
                        .child(div().text_ui(TypeSize::Small).text_color(c.text_muted).child(subtitle.to_owned())),
                ),
        )
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

/// Subtle labeled frame on a review board.
pub fn board_frame(label: &str, hint: &str, inner: AnyElement, w: Option<Pixels>, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    div()
        .when_some(w, |d, width| d.w(width).flex_none())
        .when(w.is_none(), |d| d.flex_1().min_w_0())
        .h_full()
        .flex()
        .flex_col()
        .gap(space::S2)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px(space::S1)
                .child(div().text_ui(TypeSize::Small).medium().text_color(c.text).child(label.to_owned()))
                .child(div().text_ui(TypeSize::Caption).text_color(c.text_faint).child(hint.to_owned())),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .rounded(radius::CARD)
                .overflow_hidden()
                .border_1()
                .border_color(c.border_strong)
                .child(inner),
        )
        .into_any_element()
}

/// Small keyboard-cap badge (e.g. `Esc`, `←`, `→`).
pub fn keycap(text: &str, dark_surface: bool, cx: &App) -> AnyElement {
    let c = &cx.theme().colors;
    let (bg, border, fg): (Hsla, Hsla, Hsla) = if dark_surface {
        (gpui::white().opacity(0.10), gpui::white().opacity(0.18), gpui::white().opacity(0.75))
    } else {
        (c.sunken, c.border_strong, c.text_muted)
    };
    div()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_mono(TypeSize::Caption)
        .text_color(fg)
        .child(text.to_owned())
        .into_any_element()
}
