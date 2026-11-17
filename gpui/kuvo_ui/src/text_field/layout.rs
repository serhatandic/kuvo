//! The text field's element: shapes and wraps the text, sizes the field to its lines (between
//! a minimum and a maximum, scrolling past it), and paints selection, text and caret.
//!
//! The single-line shaping, the underlined run for marked text and the input-handler
//! registration are adapted from GPUI's `examples/input.rs` (Copyright 2022 - 2025 Zed
//! Industries, Inc., Apache-2.0; the license text is in `Licenses/gpui.txt` at the repository's
//! root), and modified: wrapping, many lines, scrolling and our look are ours.

use std::ops::Range;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, ElementInputHandler, Entity, GlobalElementId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, LayoutId, MouseButton, ParentElement, Pixels, Point, SharedString, Size, Style,
    TextAlign, TextRun, UnderlineStyle, Window, WrappedLine, div, fill, point, px, relative, size,
};
use smallvec::SmallVec;

use super::{InlineBlock, TextField};
use crate::theme::{ActiveTheme, TypeSize, UI_FONT};

/// The type a field's text is set in: its size (the body's unless told otherwise) and, where it
/// has one of its own, its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FieldType {
    pub size: TypeSize,
    pub weight: Option<gpui::FontWeight>,
}

impl Default for FieldType {
    fn default() -> Self {
        FieldType { size: TypeSize::Body, weight: None }
    }
}

/// The shaped text of a field, kept from the last paint so clicks, IME and arrow keys can map
/// between positions and offsets.
pub(super) struct Layout {
    /// The text this was shaped from; a stale layout is never used.
    pub text: String,
    /// One wrapped line per paragraph (the text split at newlines).
    pub lines: SmallVec<[WrappedLine; 1]>,
    blocks: Vec<(usize, Range<usize>, Pixels)>,
    /// The byte offset each paragraph starts at.
    pub starts: Vec<usize>,
    /// The top of each paragraph, from the top of the text.
    pub tops: Vec<Pixels>,
    pub line_height: Pixels,
    /// The type it was shaped in.
    pub kind: FieldType,
    /// The width the text wraps at (`None` for one line).
    pub wrap: Option<Pixels>,
    /// Where the text's top-left is drawn, in window coordinates (scroll included).
    pub origin: Point<Pixels>,
    pub content_height: Pixels,
    /// Drawn as dots (a key): the lines are the dots', and offsets are mapped to and from the text.
    pub masked: bool,
}

/// What a masked field shows for each character.
const DOT: char = '•';

/// What a masked field draws for `text`: a dot a character.
pub(super) fn dots(text: &str) -> String {
    std::iter::repeat_n(DOT, text.chars().count()).collect()
}

impl Layout {
    /// Shapes new text where the old was drawn, for questions asked about text that changed
    /// since the last paint (an input method placing its window, an arrow key pressed twice
    /// within a frame). The next paint replaces it.
    pub fn reshape(&mut self, text: &str, marked: Option<&Range<usize>>, window: &Window) {
        self.blocks.clear();
        if text.is_empty() {
            self.lines = SmallVec::new();
            self.starts = vec![0];
            self.tops = vec![px(0.)];
            self.content_height = self.line_height;
        } else {
            let shown = if self.masked { dots(text) } else { text.to_owned() };
            let (lines, starts, tops, height) = shape(&shown, if self.masked { None } else { marked }, gpui::black(), self.wrap, self.kind, window);
            self.lines = lines;
            self.starts = starts;
            self.tops = tops;
            self.content_height = height;
        }
        self.text = text.to_string();
    }

    /// Where a text offset is in what's drawn (the same but for a masked field).
    fn shown(&self, offset: usize) -> usize {
        if self.masked { self.text[..offset.min(self.text.len())].chars().count() * DOT.len_utf8() } else { offset }
    }

    /// The text offset of an offset in what's drawn.
    fn unshown(&self, offset: usize) -> usize {
        if self.masked { self.text.char_indices().nth(offset / DOT.len_utf8()).map_or(self.text.len(), |(ix, _)| ix) } else { offset }
    }

    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset).saturating_sub(1)
    }

    /// The offsets the visual rows of a paragraph start at, local to the paragraph.
    pub fn row_starts(line: &WrappedLine) -> SmallVec<[usize; 4]> {
        let mut starts = SmallVec::new();
        starts.push(0);
        for boundary in line.wrap_boundaries.iter() {
            let run = &line.unwrapped_layout.runs[boundary.run_ix];
            starts.push(run.glyphs[boundary.glyph_ix].index);
        }
        starts
    }

    /// The caret position for an offset, relative to the text's top-left.
    pub fn position_for(&self, offset: usize) -> Point<Pixels> {
        // An empty field (showing its placeholder) has no lines: the caret is at the start.
        if self.lines.is_empty() {
            return Point::default();
        }
        let offset = self.shown(offset);
        let ix = self.line_of(offset);
        if let Some((_, range, height)) = self.blocks.iter().find(|(row, _, _)| *row == ix) {
            return point(px(0.), self.tops[ix] + if offset >= range.end { *height - self.line_height } else { px(0.) });
        }
        let local = offset.saturating_sub(self.starts[ix]).min(self.lines[ix].len());
        let at = self.lines[ix].position_for_index(local, self.line_height).unwrap_or_default();
        point(at.x, at.y + self.tops[ix])
    }

    /// The offset nearest a point relative to the text's top-left.
    pub fn offset_for(&self, at: Point<Pixels>) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        if at.y < px(0.) {
            return 0;
        }
        if at.y >= self.content_height {
            return self.text.len();
        }
        let shown = self.offset_shown(at);
        self.unshown(shown)
    }

    fn offset_shown(&self, at: Point<Pixels>) -> usize {
        let ix = self.tops.partition_point(|top| *top <= at.y).saturating_sub(1);
        if let Some((_, range, height)) = self.blocks.iter().find(|(row, _, _)| *row == ix) {
            return if at.y - self.tops[ix] < *height / 2. { range.start } else { range.end };
        }
        let line = &self.lines[ix];
        let rows = line.wrap_boundaries.len() + 1;
        let local_y = (at.y - self.tops[ix]).min(self.line_height * rows as f32 - px(0.5));
        let local = match line.closest_index_for_position(point(at.x.max(px(0.)), local_y), self.line_height) {
            Ok(i) | Err(i) => i,
        };
        self.starts[ix] + local.min(line.len())
    }

    pub fn vertical_y(&self, y: Pixels, rows: f32) -> Pixels {
        let step = self.line_height * rows;
        let mut target = y + step + self.line_height / 2.;
        for (row, _, height) in &self.blocks {
            let top = self.tops[*row];
            if rows > 0. && target >= top && target < top + *height {
                target = top + *height + self.line_height / 2.;
            } else if rows < 0. && target >= top && target < top + *height {
                target = top - self.line_height / 2.;
            }
        }
        target
    }

    /// Rectangles covering a range, one per visual row, relative to the text's top-left.
    pub fn rects_for(&self, range: &Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut rects = Vec::new();
        if range.is_empty() {
            return rects;
        }
        let range = &(self.shown(range.start)..self.shown(range.end));
        let newline_width = px(6.);
        for (ix, line) in self.lines.iter().enumerate() {
            if self.blocks.iter().any(|(row, _, _)| *row == ix) {
                continue;
            }
            let start = self.starts[ix];
            let end = start + line.len();
            if range.end < start || range.start > end {
                continue;
            }
            let row_starts = Self::row_starts(line);
            let layout = &line.unwrapped_layout;
            for (row, row_start) in row_starts.iter().enumerate() {
                let row_end = row_starts.get(row + 1).copied().unwrap_or(line.len());
                let (abs_start, abs_end) = (start + row_start, start + row_end);
                let from = range.start.max(abs_start);
                let to = range.end.min(abs_end);
                let last_row = row + 1 == row_starts.len();
                // A selection running past the end of the paragraph covers its newline too.
                let covers_newline = last_row && range.end > end;
                if from > to || (from == to && !covers_newline) {
                    continue;
                }
                let row_x = layout.x_for_index(*row_start);
                let x0 = layout.x_for_index(from - start) - row_x;
                let mut x1 = layout.x_for_index(to - start) - row_x;
                if covers_newline {
                    x1 += newline_width;
                }
                let y = self.tops[ix] + self.line_height * row as f32;
                rects.push(Bounds::new(point(x0, y), size(x1 - x0, self.line_height)));
            }
        }
        rects
    }
}

/// Shapes a field's text at a width.
pub(super) fn shape(
    text: &str,
    marked: Option<&Range<usize>>,
    color: Hsla,
    width: Option<Pixels>,
    kind: FieldType,
    window: &Window,
) -> (SmallVec<[WrappedLine; 1]>, Vec<usize>, Vec<Pixels>, Pixels) {
    shape_runs(text, marked, color, width, kind, window, &[], color)
}

fn shape_runs(
    text: &str, marked: Option<&Range<usize>>, color: Hsla, width: Option<Pixels>,
    kind: FieldType, window: &Window, links: &[Range<usize>], accent: Hsla,
) -> (SmallVec<[WrappedLine; 1]>, Vec<usize>, Vec<Pixels>, Pixels) {
    let style = window.text_style();
    let mut font = gpui::Font { family: UI_FONT.into(), ..style.font() };
    if let Some(weight) = kind.weight {
        font.weight = weight;
    }
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs: Vec<TextRun> = if !links.is_empty() {
        let mut runs = Vec::new();
        let mut at = 0;
        for link in links {
            if at < link.start { runs.push(TextRun { len: link.start - at, ..run.clone() }); }
            runs.push(TextRun {
                len: link.len(), color: accent,
                underline: Some(UnderlineStyle { color: Some(accent), thickness: px(1.), wavy: false }),
                ..run.clone()
            });
            at = link.end;
        }
        if at < text.len() { runs.push(TextRun { len: text.len() - at, ..run.clone() }); }
        runs
    } else { match marked {
        Some(marked) if !marked.is_empty() => [
            TextRun { len: marked.start, ..run.clone() },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }),
                ..run.clone()
            },
            TextRun { len: text.len() - marked.end, ..run },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        _ => vec![run],
    }};
    let line_height = kind.size.line_height();
    let lines = window
        .text_system()
        .shape_text(SharedString::from(text.to_string()), kind.size.size(), &runs, width, None)
        .unwrap_or_default();
    let mut starts = Vec::with_capacity(lines.len());
    let mut tops = Vec::with_capacity(lines.len());
    let (mut offset, mut top) = (0, px(0.));
    for line in &lines {
        starts.push(offset);
        tops.push(top);
        offset += line.len() + 1;
        top += line_height * (line.wrap_boundaries.len() + 1) as f32;
    }
    (lines, starts, tops, top)
}

type Shaped = (SmallVec<[WrappedLine; 1]>, Vec<usize>, Vec<Pixels>, Pixels);
type BlockRows = Vec<(usize, Range<usize>, Pixels)>;

fn shape_editor(
    text: &str, marked: Option<&Range<usize>>, color: Hsla, width: Option<Pixels>,
    kind: FieldType, window: &Window, blocks: &[InlineBlock], links: &[Range<usize>], accent: Hsla,
) -> (Shaped, BlockRows) {
    if blocks.is_empty() {
        return (shape_runs(text, marked, color, width, kind, window, links, accent), Vec::new());
    }
    let mut lines = SmallVec::new();
    let mut starts = Vec::new();
    let mut tops = Vec::new();
    let mut rows = Vec::new();
    let mut top = px(0.);
    let mut at = 0;
    for (end, block) in blocks.iter().map(|block| (block.range.start, Some(block))).chain([(text.len(), None)]) {
        let mut piece = &text[at..end];
        let after_block = at != 0;
        if after_block && piece.starts_with('\n') {
            at += 1;
            piece = &piece[1..];
        }
        if block.is_some() {
            piece = piece.strip_suffix('\n').unwrap_or(piece);
        }
        if !piece.is_empty() || block.is_none() {
            let local_links: Vec<_> = links.iter().filter(|link| link.start >= at && link.end <= at + piece.len())
                .map(|link| link.start - at..link.end - at).collect();
            let local_mark = marked.filter(|mark| mark.start >= at && mark.end <= at + piece.len())
                .map(|mark| mark.start - at..mark.end - at);
            let (part, offsets, heights, height) = shape_runs(piece, local_mark.as_ref(), color, width, kind, window, &local_links, accent);
            lines.extend(part);
            starts.extend(offsets.into_iter().map(|offset| at + offset));
            tops.extend(heights.into_iter().map(|height| top + height));
            top += height;
        }
        if let Some(block) = block {
            let (blank, _, _, _) = shape(" ", None, color, None, kind, window);
            rows.push((lines.len(), block.range.clone(), block.height));
            lines.extend(blank);
            starts.push(block.range.start);
            tops.push(top);
            top += block.height;
            at = block.range.end;
        }
    }
    ((lines, starts, tops, top), rows)
}

/// The element that draws a [`TextField`]'s text.
pub(super) struct TextElement {
    pub field: Entity<TextField>,
}

pub(super) struct Prepainted {
    selection: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    origin: Point<Pixels>,
    /// The placeholder, when the field is empty: it isn't part of the field's layout.
    placeholder: Option<SmallVec<[WrappedLine; 1]>>,
    line_height: Pixels,
    tops: Vec<Pixels>,
    blocks: Vec<(usize, AnyElement)>,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let field = self.field.read(cx);
        let kind = field.kind;
        let line_height = kind.size.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let Some((min_lines, max_lines)) = field.lines else {
            style.size.height = line_height.into();
            return (window.request_layout(style, [], cx), ());
        };
        let text = field.content.clone();
        let block_sizes = field.blocks().iter().map(|block| (block.range.clone(), block.height)).collect::<Vec<_>>();
        let height_limit = field.editor_height;
        let layout_id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let mut height = px(0.);
            let mut at = 0;
            for (range, block_height) in &block_sizes {
                let piece = &text[at..range.start];
                if !piece.is_empty() {
                    height += shape(piece.trim_matches('\n'), None, gpui::black(), width, kind, window).3;
                }
                height += *block_height;
                at = range.end;
            }
            let last = &text[at..];
            height += shape(if at == 0 { last } else { last.strip_prefix('\n').unwrap_or(last) }, None, gpui::black(), width, kind, window).3.max(line_height);
            let min = line_height * min_lines as f32;
            let height = match height_limit {
                Some((limit, true)) => limit.max(line_height),
                Some((limit, false)) => height.max(min).min(limit.max(line_height)),
                None => height.max(min).min(line_height * max_lines as f32),
            };
            Size { width: width.unwrap_or_default(), height }
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepainted {
        let colors = cx.theme().colors.clone();
        let focused = self.field.read(cx).focus.is_focused(window);
        let (mut painted, overlays) = self.field.update(cx, |field, _cx| {
            let multi = field.lines.is_some();
            let showing_placeholder = field.content.is_empty() && field.marked.is_none();
            let (text, color) = if showing_placeholder {
                (field.placeholder.to_string(), colors.text_faint)
            } else {
                (if field.masked { dots(&field.content) } else { field.content.clone() }, if field.disabled { colors.text_faint } else { colors.text })
            };
            let wrap = multi.then_some(bounds.size.width);
            let links: Vec<_> = field.links().iter().map(|link| link.range.clone()).collect();
            let ((lines, starts, tops, content_height), block_rows) =
                shape_editor(&text, if field.masked { None } else { field.marked.as_ref() }, color, wrap, field.kind, window, field.blocks(), &links, colors.accent);
            let line_height = field.kind.size.line_height();

            // Keep the caret in view: vertically for many lines, sideways for one.
            let first_width = lines.first().map(|l| l.width()).unwrap_or_default();
            let (lines, placeholder) = if showing_placeholder { (SmallVec::new(), Some(lines)) } else { (lines, None) };
            let mut layout = Layout {
                text: field.content.clone(),
                lines,
                blocks: block_rows,
                starts: if showing_placeholder { vec![0] } else { starts },
                tops: if showing_placeholder { vec![px(0.)] } else { tops },
                line_height,
                kind: field.kind,
                wrap,
                origin: bounds.origin,
                content_height: if showing_placeholder { line_height } else { content_height },
                masked: field.masked,
            };
            let caret_at = if showing_placeholder {
                point(px(0.), px(0.))
            } else {
                layout.position_for(field.cursor())
            };
            if multi {
                let max_scroll = (layout.content_height - bounds.size.height).max(px(0.));
                if field.reveal_caret {
                    if caret_at.y < field.scroll {
                        field.scroll = caret_at.y;
                    } else if caret_at.y + line_height > field.scroll + bounds.size.height {
                        field.scroll = caret_at.y + line_height - bounds.size.height;
                    }
                }
                field.scroll = field.scroll.clamp(px(0.), max_scroll);
                layout.origin = point(bounds.origin.x, bounds.origin.y - field.scroll);
            } else {
                let max_scroll = (first_width + px(2.) - bounds.size.width).max(px(0.));
                if field.reveal_caret {
                    if caret_at.x < field.scroll {
                        field.scroll = caret_at.x;
                    } else if caret_at.x + px(2.) > field.scroll + bounds.size.width {
                        field.scroll = caret_at.x + px(2.) - bounds.size.width;
                    }
                }
                field.scroll = field.scroll.clamp(px(0.), max_scroll);
                layout.origin = point(bounds.origin.x - field.scroll, bounds.origin.y);
            }
            field.reveal_caret = false;

            let origin = layout.origin;
            let selection = if showing_placeholder {
                Vec::new()
            } else {
                layout
                    .rects_for(&field.selection)
                    .into_iter()
                    .map(|r| Bounds::new(r.origin + origin, r.size))
                    .collect()
            };
            let caret = (focused && window.is_window_active() && field.caret_visible && field.selection.is_empty() && !field.disabled).then(|| {
                Bounds::new(
                    point(origin.x + caret_at.x - px(0.5), origin.y + caret_at.y + px(1.)),
                    size(px(2.), line_height - px(2.)),
                )
            });
            let overlays = layout.blocks.iter().zip(field.blocks()).filter_map(|((row, _, height), block)| {
                let top = origin.y + layout.tops[*row];
                (top < bounds.bottom() && top + *height > bounds.top()).then(|| (*row, top, *height, block.draw.clone()))
            }).collect::<Vec<_>>();
            let tops = layout.tops.clone();
            field.layout = Some(layout);
            (Prepainted { selection, caret, origin, placeholder, line_height, tops, blocks: Vec::new() }, overlays)
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (row, top, height, draw) in overlays {
                let mut block = div()
                    .id(("field-inline-block", row))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(draw(bounds.size.width, cx))
                    .into_any_element();
                block.prepaint_as_root(point(painted.origin.x, top), size(AvailableSpace::Definite(bounds.size.width), AvailableSpace::Definite(height)), window, cx);
                painted.blocks.push((row, block));
            }
        });
        painted
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepainted: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let colors = cx.theme().colors.clone();
        let focus = self.field.read(cx).focus.clone();
        // A field that isn't focused keeps its selection, greyed, as macOS does.
        let selection_color = if focus.is_focused(window) { colors.selection } else { colors.pressed };
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for rect in prepainted.selection.drain(..) {
                window.paint_quad(fill(rect, selection_color));
            }
            // Paint the field's own lines, lent out of the field for the length of the paint.
            let laid_out = self.field.update(cx, |field, _| field.layout.as_mut().map(|l| std::mem::take(&mut l.lines)));
            let placeholder = prepainted.placeholder.take();
            let lent = placeholder.is_none();
            let lines = placeholder.or(laid_out).unwrap_or_default();
            let mut top = prepainted.origin.y;
            for (row, line) in lines.iter().enumerate() {
                if lent {
                    top = prepainted.origin.y + prepainted.tops[row];
                    let is_block = self.field.read(cx).layout.as_ref().is_some_and(|layout| layout.blocks.iter().any(|(ix, _, _)| *ix == row));
                    if is_block { continue; }
                }
                let rows = line.wrap_boundaries.len() + 1;
                line.paint(
                    point(prepainted.origin.x, top),
                    prepainted.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
                top += prepainted.line_height * rows as f32;
            }
            if lent {
                self.field.update(cx, |field, _| {
                    if let Some(layout) = field.layout.as_mut() {
                        layout.lines = lines;
                    }
                });
            }
            for (_, block) in &mut prepainted.blocks {
                block.paint(window, cx);
            }
            if let Some(caret) = prepainted.caret.take() {
                window.paint_quad(fill(caret, colors.accent).corner_radii(px(1.)));
            }
        });
    }
}
