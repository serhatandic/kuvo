//! A terminal's grid, drawn as one element: as many cells as fit (the app's mono font, each cell
//! as wide as its "m"), each row's text shaped once with every glyph one cell wide (a wide or
//! combined character on its own), the cells' own backgrounds, the selection and the cursor.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors as Overrides;
use alacritty_terminal::term::{Term, point_to_viewport};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::{
    App, BorderStyle, Bounds, ContentMask, Element, ElementId, Entity, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    LayoutId, Pixels, Point, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window,
    fill, outline, point, px, relative, size,
};
use kuvo_ui::theme::{MONO_FONT, TypeSize, scaled};
use kuvo_ui::ActiveTheme;

use super::palette::{Palette, dim};
use super::{Asked, Layout, TerminalView};

/// The terminal's type: the app's mono font at the size its output views use, its lines a little
/// taller than the type, as Terminal's are.
fn metrics() -> (Pixels, Pixels) {
    (TypeSize::Caption.size(), scaled(px(16.)))
}

pub(crate) struct TerminalElement {
    view: Entity<TerminalView>,
    /// Its shell runs: the cursor is drawn.
    running: bool,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, running: bool) -> TerminalElement {
        TerminalElement { view, running }
    }
}

/// How one cell is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Look {
    pub fg: Hsla,
    /// None: the panel's surface, unpainted.
    pub bg: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub curly: bool,
    pub strike: bool,
}

pub(crate) fn look(cell: &Cell, palette: &Palette, overrides: &Overrides) -> Look {
    let flags = cell.flags;
    let mut fg = palette.resolve(cell.fg, overrides);
    let mut bg = match cell.bg {
        Color::Named(NamedColor::Background) if overrides[NamedColor::Background].is_none() => None,
        color => Some(palette.resolve(color, overrides)),
    };
    if flags.contains(Flags::DIM) {
        fg = dim(fg);
    }
    if flags.contains(Flags::INVERSE) {
        let behind = bg.unwrap_or(palette.background);
        bg = Some(fg);
        fg = behind;
    }
    Look {
        fg,
        bg,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        curly: flags.contains(Flags::UNDERCURL),
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

/// The text a cell shows: its character and those combined with it; a hidden one's a blank.
fn shown(cell: &Cell) -> String {
    if cell.flags.contains(Flags::HIDDEN) {
        return " ".into();
    }
    let mut text = String::from(cell.c);
    if let Some(combined) = cell.zerowidth() {
        text.extend(combined);
    }
    text
}

/// Whether the cell is the second half of a wide character, drawn by the first.
fn spacer(cell: &Cell) -> bool {
    cell.flags.contains(Flags::WIDE_CHAR_SPACER)
}

/// The screen as text, a line a row, without trailing blanks.
pub(crate) fn screen_text(term: &Term<Asked>) -> Vec<String> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    (0..grid.screen_lines())
        .map(|row| {
            let line = Line(row as i32 - offset);
            let text: String = (0..grid.columns()).map(|column| &grid[line][Column(column)]).filter(|cell| !spacer(cell)).map(shown).collect();
            text.trim_end().to_owned()
        })
        .collect()
}

/// The colour of the character at `row` and `column` of the screen.
pub(crate) fn color_at(term: &Term<Asked>, palette: &Palette, row: usize, column: usize) -> Option<Hsla> {
    let grid = term.grid();
    if row >= grid.screen_lines() || column >= grid.columns() {
        return None;
    }
    let line = Line(row as i32 - grid.display_offset() as i32);
    Some(look(&grid[line][Column(column)], palette, term.colors()).fg)
}

/// A piece of a row shaped together: from `column`, every glyph one cell wide, or `width` cells for
/// a wide character.
struct Segment {
    column: usize,
    text: String,
    runs: Vec<(usize, Look)>,
    width: Option<usize>,
}

impl Segment {
    fn new(column: usize) -> Segment {
        Segment { column, text: String::new(), runs: Vec::new(), width: Some(1) }
    }

    fn push(&mut self, text: &str, look: Look) {
        self.text.push_str(text);
        match self.runs.last_mut() {
            Some((length, last)) if *last == look => *length += text.len(),
            _ => self.runs.push((text.len(), look)),
        }
    }

    /// Without its trailing blanks that draw nothing.
    fn trimmed(mut self) -> Option<Segment> {
        while self.text.ends_with(' ') {
            let Some((length, look)) = self.runs.last_mut() else { break };
            if look.underline || look.strike {
                break;
            }
            self.text.pop();
            *length -= 1;
            if *length == 0 {
                self.runs.pop();
            }
        }
        (!self.text.is_empty()).then_some(self)
    }
}

/// What a row draws: its text in segments, and its cells' backgrounds (from a column, so many
/// columns wide).
struct Row {
    segments: Vec<Segment>,
    backgrounds: Vec<(usize, usize, Hsla)>,
}

fn rows(term: &Term<Asked>, palette: &Palette) -> Vec<Row> {
    let grid = term.grid();
    let offset = grid.display_offset() as i32;
    let overrides = term.colors();
    (0..grid.screen_lines())
        .map(|row| {
            let line = Line(row as i32 - offset);
            let mut segments = Vec::new();
            let mut backgrounds: Vec<(usize, usize, Hsla)> = Vec::new();
            let mut current = Segment::new(0);
            for column in 0..grid.columns() {
                let cell = &grid[line][Column(column)];
                if spacer(cell) {
                    continue;
                }
                let look = look(cell, palette, overrides);
                let wide = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                if let Some(color) = look.bg {
                    match backgrounds.last_mut() {
                        Some((start, width, last)) if *last == color && *start + *width == column => *width += wide,
                        _ => backgrounds.push((column, wide, color)),
                    }
                }
                let text = shown(cell);
                if wide == 2 || cell.zerowidth().is_some() {
                    segments.extend(std::mem::replace(&mut current, Segment::new(column + wide)).trimmed());
                    let mut own = Segment::new(column);
                    own.width = (wide == 2).then_some(2);
                    own.push(&text, look);
                    segments.push(own);
                    continue;
                }
                if current.text.is_empty() {
                    current.column = column;
                }
                current.push(&text, look);
            }
            segments.extend(current.trimmed());
            Row { segments, backgrounds }
        })
        .collect()
}

/// What's painted, laid out.
pub(crate) struct Painted {
    line_height: Pixels,
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    selection: Vec<Bounds<Pixels>>,
    selection_color: Hsla,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<Cursor>,
}

struct Cursor {
    bounds: Bounds<Pixels>,
    color: Hsla,
    hollow: bool,
    /// The character under a block cursor, drawn on it.
    text: Option<(Point<Pixels>, ShapedLine)>,
}

fn run(font: &Font, length: usize, look: Look, color: Hsla) -> TextRun {
    TextRun {
        len: length,
        font: Font { weight: if look.bold { FontWeight::BOLD } else { font.weight }, style: if look.italic { FontStyle::Italic } else { FontStyle::Normal }, ..font.clone() },
        color,
        background_color: None,
        underline: look.underline.then_some(UnderlineStyle { thickness: px(1.), color: Some(look.fg), wavy: look.curly }),
        strikethrough: look.strike.then_some(StrikethroughStyle { thickness: px(1.), color: Some(look.fg) }),
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Painted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Painted {
        let (font_size, line_height) = metrics();
        let font = gpui::font(MONO_FONT);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system.advance(font_id, font_size, 'm').map(|advance| advance.width).unwrap_or(font_size * 0.6);
        let cols = ((f32::from(bounds.size.width) / f32::from(cell_width)).floor() as usize).max(2);
        let lines = ((f32::from(bounds.size.height) / f32::from(line_height)).floor() as usize).max(1);
        let layout = Layout { origin: bounds.origin, cell_width, line_height, rows: lines, cols };
        self.view.update(cx, |view, cx| view.laid_out(layout, cx));

        let palette = Palette::of(cx.theme());
        let view = self.view.read(cx);
        let focused = view.focus.is_focused(window) && window.is_window_active();
        let term = &view.term;
        let at = |row: usize, column: usize| point(bounds.origin.x + cell_width * column as f32, bounds.origin.y + line_height * row as f32);
        let mut painted = Painted { line_height, backgrounds: Vec::new(), selection: Vec::new(), selection_color: palette.selection, lines: Vec::new(), cursor: None };

        for (row, drawn) in rows(term, &palette).into_iter().enumerate() {
            for (column, width, color) in drawn.backgrounds {
                painted.backgrounds.push((Bounds::new(at(row, column), size(cell_width * width as f32, line_height)), color));
            }
            for segment in drawn.segments {
                let runs: Vec<TextRun> = segment.runs.iter().map(|(length, look)| run(&font, *length, *look, look.fg)).collect();
                let force = segment.width.map(|cells| cell_width * cells as f32);
                let shaped = text_system.shape_line(SharedString::from(segment.text), font_size, &runs, force);
                painted.lines.push((at(row, segment.column), shaped));
            }
        }

        // The selection, row by row.
        let offset = term.grid().display_offset() as i32;
        if let Some(range) = term.selection.as_ref().and_then(|selection| selection.to_range(term)) {
            for row in 0..lines {
                let line = Line(row as i32 - offset);
                if line < range.start.line || line > range.end.line {
                    continue;
                }
                let (from, to) = if range.is_block {
                    (range.start.column.0, range.end.column.0)
                } else {
                    (if line == range.start.line { range.start.column.0 } else { 0 }, if line == range.end.line { range.end.column.0 } else { cols - 1 })
                };
                if to >= from {
                    painted.selection.push(Bounds::new(at(row, from), size(cell_width * (to - from + 1) as f32, line_height)));
                }
            }
        }

        // The cursor: a block where you type, hollow while the terminal isn't where the keys go.
        let cursor = term.renderable_content().cursor;
        let shape = cursor.shape;
        if self.running && shape != CursorShape::Hidden && let Some(spot) = point_to_viewport(offset as usize, cursor.point) {
            let cell = &term.grid()[cursor.point];
            let wide = if cell.flags.contains(Flags::WIDE_CHAR) { 2. } else { 1. };
            let origin = at(spot.line, spot.column.0);
            let block = Bounds::new(origin, size(cell_width * wide, line_height));
            let (bounds, hollow) = match shape {
                CursorShape::Beam => (Bounds::new(origin, size(px(2.), line_height)), false),
                CursorShape::Underline => (Bounds::new(point(origin.x, origin.y + line_height - px(2.)), size(cell_width * wide, px(2.))), false),
                CursorShape::HollowBlock => (block, true),
                _ => (block, !focused),
            };
            let text = (shape == CursorShape::Block && focused && cell.c != ' ').then(|| {
                let look = look(cell, &palette, term.colors());
                let text = shown(cell);
                let runs = [run(&font, text.len(), look, palette.on_cursor)];
                (origin, text_system.shape_line(SharedString::from(text), font_size, &runs, Some(cell_width * wide)))
            });
            painted.cursor = Some(Cursor { bounds, color: palette.cursor, hollow, text });
        }
        painted
    }

    fn paint(&mut self, _id: Option<&GlobalElementId>, _inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), painted: &mut Painted, window: &mut Window, cx: &mut App) {
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (rect, color) in painted.backgrounds.drain(..) {
                window.paint_quad(fill(rect, color));
            }
            for rect in painted.selection.drain(..) {
                window.paint_quad(fill(rect, painted.selection_color));
            }
            for (origin, line) in &painted.lines {
                line.paint(*origin, painted.line_height, TextAlign::Left, None, window, cx).ok();
            }
            if let Some(cursor) = painted.cursor.take() {
                if cursor.hollow {
                    window.paint_quad(outline(cursor.bounds, cursor.color, BorderStyle::Solid));
                } else {
                    window.paint_quad(fill(cursor.bounds, cursor.color));
                }
                if let Some((origin, text)) = cursor.text {
                    text.paint(origin, painted.line_height, TextAlign::Left, None, window, cx).ok();
                }
            }
        });
    }
}
