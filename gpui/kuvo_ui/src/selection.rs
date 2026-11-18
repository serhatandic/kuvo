//! Selecting and copying text across a conversation (`gpui/notes/selection.md`).
//!
//! One [`SelectionState`] holds the selection; nothing else does. Text that can be selected is
//! drawn by a [`SelectableText`], which tells the state where it was laid out this frame and
//! paints its part of the selection. A [`SelectionArea`] around the list takes the mouse: press,
//! drag, double and triple clicks, shift-clicks, and scrolling while a drag is past an edge.
//!
//! A piece of selectable text is a unit, known by its row (a message's key) and a path in reading
//! order, so a selection is about the conversation's text, not about what's on screen: it lasts
//! while text streams in, rows scroll away and earlier ones load above.

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::HashMap,
    ops::Range,
    rc::Rc,
};

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, InteractiveText, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString,
    StyledText, TextLayout, TextRun, Window, point, px, quad, size,
};
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::ActiveTheme;

/// The same Copy and Select All as a text field's, so the menu bar's Edit items reach a
/// transcript's selection as they reach a field.
pub use crate::text_field::{Copy, SelectAll};

/// The key context of a selectable transcript, for its ⌘C and ⌘A.
pub const CONTEXT: &str = "KuvoSelection";

/// The keys a transcript answers to. Called from [`crate::bind_keys`].
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    #[cfg(target_os = "macos")]
    cx.bind_keys([gpui::KeyBinding::new("cmd-c", Copy, c), gpui::KeyBinding::new("cmd-a", SelectAll, c)]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([gpui::KeyBinding::new("ctrl-c", Copy, c), gpui::KeyBinding::new("ctrl-a", SelectAll, c)]);
}

/// How Copy and Select All's keys are written in a menu, on this system.
pub const COPY_KEYS: &str = if cfg!(target_os = "macos") { "⌘C" } else { "Ctrl+C" };
pub const SELECT_ALL_KEYS: &str = if cfg!(target_os = "macos") { "⌘A" } else { "Ctrl+A" };

/// How far the pointer goes, pressed, before it's a drag rather than a click (a hand on a trackpad
/// wobbles a little).
const DRAG_THRESHOLD: f32 = 3.;

/// Where a unit sits in its row, in reading order (the content part, then the block, the item…).
pub type Path = SmallVec<[u32; 6]>;

/// A piece of selectable text: its row's key and its path in the row.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UnitId {
    pub row: SharedString,
    pub path: Path,
}

impl UnitId {
    pub fn new(row: impl Into<SharedString>, path: &[u32]) -> Self {
        Self { row: row.into(), path: Path::from_slice(path) }
    }
}

/// A place in the text: a unit, and a byte offset in it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Spot {
    pub unit: UnitId,
    pub offset: usize,
}

impl Spot {
    pub fn new(unit: UnitId, offset: usize) -> Self {
        Self { unit, offset }
    }

    /// Before everything in `row`.
    pub fn row_start(row: impl Into<SharedString>) -> Self {
        Self { unit: UnitId { row: row.into(), path: Path::new() }, offset: 0 }
    }

    /// After everything in `row`.
    pub fn row_end(row: impl Into<SharedString>) -> Self {
        Self { unit: UnitId { row: row.into(), path: Path::from_slice(&[u32::MAX]) }, offset: usize::MAX }
    }
}

/// How a unit joins the one before it when copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lead {
    /// A blank line: a new paragraph, heading or block.
    Paragraph,
    /// A new line in the same block (a table's row, a list item's next paragraph).
    Line,
    /// A tab: the next cell in a table's row.
    Cell,
    /// A new line starting a list item, with its marker (`- `, `1. `, `- [x] `), indented two
    /// spaces for each level it's nested.
    Item { depth: usize, marker: String },
}

/// A unit's text as copying sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub path: Path,
    pub text: String,
    pub lead: Lead,
}

impl Unit {
    pub fn new(path: &[u32], text: impl Into<String>, lead: Lead) -> Self {
        Self { path: Path::from_slice(path), text: text.into(), lead }
    }
}

/// The order of the rows, by key.
#[derive(Clone, Debug, Default)]
pub struct Order(HashMap<SharedString, usize>);

impl Order {
    pub fn new<'a>(keys: impl IntoIterator<Item = &'a str>) -> Self {
        Self(keys.into_iter().enumerate().map(|(ix, key)| (SharedString::from(key.to_owned()), ix)).collect())
    }

    pub fn has(&self, row: &str) -> bool {
        self.0.contains_key(row)
    }

    /// Spots in reading order; none for a spot in a row that isn't there.
    pub fn cmp(&self, a: &Spot, b: &Spot) -> Option<Ordering> {
        let (ra, rb) = (self.0.get(&a.unit.row)?, self.0.get(&b.unit.row)?);
        Some(ra.cmp(rb).then_with(|| a.unit.path.cmp(&b.unit.path)).then_with(|| a.offset.cmp(&b.offset)))
    }

    fn cmp_unit(&self, a: &UnitId, b: &UnitId) -> Option<Ordering> {
        let (ra, rb) = (self.0.get(&a.row)?, self.0.get(&b.row)?);
        Some(ra.cmp(rb).then_with(|| a.path.cmp(&b.path)))
    }
}

/// The part of a unit `len` long between `start` and `end` (in order), if any of it.
pub fn range_in(order: &Order, unit: &UnitId, len: usize, start: &Spot, end: &Spot) -> Option<Range<usize>> {
    let from = match order.cmp_unit(unit, &start.unit)? {
        Ordering::Less => return None,
        Ordering::Equal => start.offset.min(len),
        Ordering::Greater => 0,
    };
    let to = match order.cmp_unit(unit, &end.unit)? {
        Ordering::Greater => return None,
        Ordering::Equal => end.offset.min(len),
        Ordering::Less => len,
    };
    (from < to || (from == to && unit != &end.unit && len == 0)).then_some(from..to)
}

/// The text between `start` and `end` (in order), from the rows' units (each row's in reading
/// order), as it's copied: blocks joined as their leads say, rows by a blank line.
pub fn copy_text(order: &Order, rows: impl IntoIterator<Item = (SharedString, Vec<Unit>)>, start: &Spot, end: &Spot) -> String {
    let mut out = String::new();
    for (row, units) in rows {
        let mut row_text = String::new();
        // Whether the last unit copied was in a list: a list starts after a blank line, its items
        // follow each other line by line.
        let mut in_list = false;
        for unit in units {
            let id = UnitId { row: row.clone(), path: unit.path.clone() };
            let Some(range) = range_in(order, &id, unit.text.len(), start, end) else { continue };
            let range = snap(&unit.text, range);
            if !row_text.is_empty() {
                match &unit.lead {
                    Lead::Paragraph => row_text.push_str("\n\n"),
                    Lead::Line => row_text.push('\n'),
                    Lead::Cell => row_text.push('\t'),
                    Lead::Item { depth, marker } => {
                        row_text.push_str(if in_list { "\n" } else { "\n\n" });
                        row_text.push_str(&"  ".repeat(*depth));
                        // A marker goes with the item only when its text is taken from the start.
                        if range.start == 0 {
                            row_text.push_str(marker);
                        }
                    }
                }
            } else if let (Lead::Item { depth, marker }, 0) = (&unit.lead, range.start) {
                // The first thing copied, a list item from its start: it keeps its marker.
                row_text.push_str(&"  ".repeat(*depth));
                row_text.push_str(marker);
            }
            row_text.push_str(&unit.text[range]);
            in_list = match unit.lead {
                Lead::Item { .. } => true,
                Lead::Line => in_list,
                _ => false,
            };
        }
        if row_text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&row_text);
    }
    out
}

/// `range` moved onto `text`'s character boundaries.
fn snap(text: &str, range: Range<usize>) -> Range<usize> {
    let floor = |mut ix: usize| {
        ix = ix.min(text.len());
        while !text.is_char_boundary(ix) {
            ix -= 1;
        }
        ix
    };
    floor(range.start)..floor(range.end)
}

/// The word around `offset` (Unicode's word bounds, as the composer's), or the run of spaces or
/// punctuation it's in.
pub fn word_around(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let mut found = None;
    for (ix, word) in text.split_word_bound_indices() {
        if ix <= offset && offset < ix + word.len() {
            found = Some(ix..ix + word.len());
            break;
        }
        // At the very end: the last word.
        if ix + word.len() == offset && offset == text.len() {
            found = Some(ix..offset);
        }
    }
    found.unwrap_or(offset..offset)
}

/// The line around `offset`: from the hard break before it to the one after (a paragraph is one
/// line until it has a hard break; in code, a line of code).
pub fn line_around(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |ix| ix + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |ix| offset + ix);
    start..end
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Character,
    /// Extending by whole words or lines from the one first picked.
    Word(Spot, Spot),
    Line(Spot, Spot),
}

#[derive(Clone, Debug)]
struct Selection {
    anchor: Spot,
    head: Spot,
    mode: Mode,
}

/// A unit laid out in this frame: where, and how to find a character in it.
#[derive(Clone)]
struct Laid {
    unit: UnitId,
    layout: TextLayout,
    /// Where it shows (the part of it not scrolled out of its block).
    visible: Bounds<Pixels>,
    text: SharedString,
    links: Rc<Vec<(Range<usize>, String)>>,
}

#[derive(Default)]
struct Inner {
    selection: Option<Selection>,
    /// The button's down and dragging.
    pending: bool,
    /// The pointer while dragging, for scrolling past an edge.
    pointer: Option<Point<Pixels>>,
    /// Whether this press selected anything (a drag, a double click): a link pressed then doesn't
    /// open.
    press_selected: bool,
    /// The row last pressed in, for ⌘A.
    last_row: Option<SharedString>,
    /// Where the button went down, and, for a press off the text (a step's row, a button, the
    /// margin), the spot it starts a selection at once it's dragged.
    origin: Option<Point<Pixels>>,
    deferred: Option<Spot>,
    /// Whether the pointer has moved far enough since the press to be a drag, not a click.
    moved: bool,
    order: Order,
    laid: Vec<Laid>,
    /// Whether the selection is the one in use (its transcript has the keyboard).
    emphasized: bool,
    /// Whether the button came up on a selection (a drag, a double or triple click, a shift-click)
    /// and nothing since has pressed, cleared or replaced it: the moment "Add to Chat" shows near
    /// its end.
    finished: bool,
    /// Where the end of the selection was drawn when the card over it was last placed.
    card_at: Option<Bounds<Pixels>>,
    /// Whether its transcript puts a card over it at all (a group chat's doesn't yet).
    card_used: bool,
}

/// A transcript's selection. Cheap to clone; clones share it.
#[derive(Clone, Default)]
pub struct SelectionState(Rc<RefCell<Inner>>);

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows, in order, by key. A selection whose ends' rows are gone goes with them.
    pub fn set_rows<'a>(&self, keys: impl IntoIterator<Item = &'a str>) {
        let mut inner = self.0.borrow_mut();
        inner.order = Order::new(keys);
        let gone = inner.selection.as_ref().is_some_and(|s| !inner.order.has(&s.anchor.unit.row) || !inner.order.has(&s.head.unit.row));
        if gone {
            inner.selection = None;
            inner.pending = false;
            inner.finished = false;
        }
    }

    pub fn order(&self) -> Order {
        self.0.borrow().order.clone()
    }

    /// The selection's ends in reading order, if it holds any text.
    pub fn range(&self) -> Option<(Spot, Spot)> {
        let inner = self.0.borrow();
        let selection = inner.selection.as_ref()?;
        match inner.order.cmp(&selection.anchor, &selection.head)? {
            Ordering::Less => Some((selection.anchor.clone(), selection.head.clone())),
            Ordering::Greater => Some((selection.head.clone(), selection.anchor.clone())),
            Ordering::Equal => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.range().is_none()
    }

    pub fn clear(&self) {
        let mut inner = self.0.borrow_mut();
        inner.selection = None;
        inner.pending = false;
        inner.finished = false;
    }

    /// Selects from `anchor` to `head`.
    pub fn select(&self, anchor: Spot, head: Spot) {
        self.0.borrow_mut().selection = Some(Selection { anchor, head, mode: Mode::Character });
    }

    /// ⌘A: the row you're in (where the selection is, or you last pressed); again, every row.
    /// `first` and `last`: the first and last rows' keys.
    pub fn select_all(&self, first: &str, last: &str) {
        let range = self.range();
        let row = {
            let inner = self.0.borrow();
            inner.selection.as_ref().map(|s| s.head.unit.row.clone()).or_else(|| inner.last_row.clone())
        };
        let whole_row = |row: &SharedString| range.as_ref().is_some_and(|(start, end)| *start == Spot::row_start(row.clone()) && *end == Spot::row_end(row.clone()));
        match row {
            Some(row) if self.0.borrow().order.has(&row) && !whole_row(&row) => self.select(Spot::row_start(row.clone()), Spot::row_end(row)),
            _ => self.select(Spot::row_start(first.to_owned()), Spot::row_end(last.to_owned())),
        }
    }

    /// Whether the last press selected something, rather than clicking.
    pub fn press_selected(&self) -> bool {
        self.0.borrow().press_selected
    }

    pub fn is_dragging(&self) -> bool {
        self.0.borrow().pending
    }

    /// Whether a selection has just been finished (the button came up on it): it's what the
    /// "Add to Chat" card over its end shows for, until the selection clears, a press lands
    /// elsewhere, or it's used.
    pub fn is_finished(&self) -> bool {
        let inner = self.0.borrow();
        inner.finished && !inner.pending && inner.selection.is_some()
    }

    /// Hides what shows for a finished selection, leaving the selection as it is.
    pub fn dismiss_finished(&self) {
        self.0.borrow_mut().finished = false;
    }

    /// Where the end of the selection (in reading order) was drawn this frame: its line's height,
    /// at the end character's left edge. None if there's no selection, or its end isn't on screen.
    pub fn end_bounds(&self) -> Option<Bounds<Pixels>> {
        let (_, end) = self.range()?;
        let inner = self.0.borrow();
        let laid = inner.laid.iter().find(|laid| laid.unit == end.unit)?;
        let at = laid.layout.position_for_index(end.offset.min(laid.text.len()))?;
        let line = Bounds { origin: point(at.x, at.y), size: size(px(0.), laid.layout.line_height()) };
        (laid.visible.contains(&line.origin) || laid.visible.contains(&point(at.x, at.y + line.size.height / 2.))).then_some(line)
    }

    /// The end's bounds for placing the card over it, remembered so a frame that draws it
    /// elsewhere (the transcript scrolled) is seen, and drawn again.
    pub fn card_anchor(&self) -> Option<Bounds<Pixels>> {
        let at = if self.is_finished() { self.end_bounds() } else { None };
        {
            let mut inner = self.0.borrow_mut();
            inner.card_at = at;
            inner.card_used = true;
        }
        at
    }

    /// Where the character at `offset` of `unit` was drawn this frame (the middle of its line's
    /// height at its left edge), if it was: for tests and shots to press on.
    pub fn point_for(&self, unit: &UnitId, offset: usize) -> Option<Point<Pixels>> {
        let inner = self.0.borrow();
        let laid = inner.laid.iter().find(|laid| laid.unit == *unit)?;
        let at = laid.layout.position_for_index(offset.min(laid.text.len()))?;
        Some(point(at.x + px(1.), at.y + laid.layout.line_height() / 2.))
    }

    /// The link at `position`, among the text laid out in this frame.
    pub fn link_at(&self, position: Point<Pixels>) -> Option<String> {
        let inner = self.0.borrow();
        let laid = inner.laid.iter().find(|laid| laid.visible.contains(&position))?;
        let ix = index_at(&laid.layout, position)?;
        laid.links.iter().find(|(range, _)| range.contains(&ix)).map(|(_, url)| url.clone())
    }

    /// The spot under `position` (or nearest it) among the text laid out in this frame: the unit
    /// under it; else the one on its line, the nearer side; else, in a gap, the end of the unit
    /// above; above everything, the start of the first.
    pub fn spot_at(&self, position: Point<Pixels>) -> Option<Spot> {
        let inner = self.0.borrow();
        let spot = |laid: &Laid, at: Point<Pixels>| index_at(&laid.layout, at).map(|ix| Spot::new(laid.unit.clone(), ix.min(laid.text.len())));
        if let Some(laid) = inner.laid.iter().find(|laid| laid.visible.contains(&position)) {
            return spot(laid, position);
        }
        // On the same line as some text: the nearest of those.
        let beside = inner
            .laid
            .iter()
            .filter(|laid| laid.visible.top() <= position.y && position.y < laid.visible.bottom())
            .min_by(|a, b| {
                let gap = |laid: &&Laid| (laid.visible.left() - position.x).max(position.x - laid.visible.right()).max(px(0.));
                gap(a).partial_cmp(&gap(b)).unwrap_or(Ordering::Equal)
            });
        if let Some(laid) = beside {
            let x = position.x.clamp(laid.visible.left(), laid.visible.right() - px(0.5));
            return spot(laid, point(x, position.y));
        }
        let above = inner.laid.iter().filter(|laid| laid.visible.bottom() <= position.y).max_by(|a, b| a.visible.bottom().partial_cmp(&b.visible.bottom()).unwrap_or(Ordering::Equal).then_with(|| inner.order.cmp_unit(&a.unit, &b.unit).unwrap_or(Ordering::Equal)));
        match above {
            Some(laid) => Some(Spot::new(laid.unit.clone(), laid.text.len())),
            None => inner.laid.first().map(|laid| Spot::new(laid.unit.clone(), 0)),
        }
    }

    fn laid(&self, unit: &UnitId) -> Option<SharedString> {
        self.0.borrow().laid.iter().find(|laid| laid.unit == *unit).map(|laid| laid.text.clone())
    }

    /// A press at `spot`: a click places it (shift: extends to it), a double click takes the word,
    /// a triple click the line.
    pub fn press(&self, spot: Spot, clicks: usize, shift: bool) {
        let text = self.laid(&spot.unit).unwrap_or_default();
        let mut inner = self.0.borrow_mut();
        inner.pending = true;
        inner.finished = false;
        inner.press_selected = clicks > 1 || shift;
        inner.last_row = Some(spot.unit.row.clone());
        let within = |range: Range<usize>| (Spot::new(spot.unit.clone(), range.start), Spot::new(spot.unit.clone(), range.end));
        inner.selection = Some(match (clicks, &inner.selection) {
            (1, Some(old)) if shift => Selection { anchor: old.anchor.clone(), head: spot, mode: Mode::Character },
            (1, _) => Selection { anchor: spot.clone(), head: spot, mode: Mode::Character },
            (2, _) => {
                let (start, end) = within(word_around(&text, spot.offset));
                Selection { anchor: start.clone(), head: end.clone(), mode: Mode::Word(start, end) }
            }
            _ => {
                let (start, end) = within(line_around(&text, spot.offset));
                Selection { anchor: start.clone(), head: end.clone(), mode: Mode::Line(start, end) }
            }
        });
    }

    /// The drag reached `spot`: the selection runs from where it began to there, by whole words
    /// or lines after a double or triple click.
    pub fn drag_to(&self, spot: Spot) {
        let text = self.laid(&spot.unit).unwrap_or_default();
        let mut inner = self.0.borrow_mut();
        let order = inner.order.clone();
        let Some(selection) = inner.selection.as_mut() else { return };
        match selection.mode.clone() {
            Mode::Character => selection.head = spot,
            Mode::Word(start, end) | Mode::Line(start, end) => {
                let by_word = matches!(selection.mode, Mode::Word(..));
                let around = if by_word { word_around(&text, spot.offset) } else { line_around(&text, spot.offset) };
                if order.cmp(&spot, &start) == Some(Ordering::Less) {
                    selection.anchor = end;
                    selection.head = Spot::new(spot.unit, around.start);
                } else if order.cmp(&spot, &end) == Some(Ordering::Greater) {
                    selection.anchor = start;
                    selection.head = Spot::new(spot.unit, around.end);
                } else {
                    selection.anchor = start;
                    selection.head = end;
                }
            }
        }
        let moved = inner.selection.as_ref().is_some_and(|s| s.anchor != s.head);
        if moved {
            inner.press_selected = true;
        }
    }

    /// The button came up. (A click that selected nothing leaves only where it was, for a
    /// shift-click to extend from.)
    pub fn release(&self) {
        let mut inner = self.0.borrow_mut();
        let selected = inner.selection.as_ref().is_some_and(|s| s.anchor != s.head);
        inner.finished = inner.pending && selected;
        inner.pending = false;
        inner.pointer = None;
        inner.origin = None;
        inner.deferred = None;
    }

    /// A press off the text, at `spot`'s nearest text: nothing changes unless it's dragged (a click
    /// on a fold or a button leaves the selection as it was).
    fn press_off_text(&self, spot: Spot) {
        let mut inner = self.0.borrow_mut();
        inner.pending = true;
        inner.finished = false;
        inner.press_selected = false;
        inner.deferred = Some(spot);
    }

    /// The pointer, pressed, is at `position`: whether that's a drag yet (past a few pixels from
    /// the press, as a Mac waits for), starting a press off the text if it is.
    fn drag_begins(&self, position: Point<Pixels>) -> bool {
        let deferred = {
            let mut inner = self.0.borrow_mut();
            inner.pointer = Some(position);
            if !inner.moved {
                let far = inner.origin.is_none_or(|origin| {
                    let (dx, dy) = (f32::from(position.x - origin.x), f32::from(position.y - origin.y));
                    dx * dx + dy * dy >= DRAG_THRESHOLD * DRAG_THRESHOLD
                });
                if !far {
                    return false;
                }
                inner.moved = true;
            }
            inner.deferred.take()
        };
        if let Some(spot) = deferred {
            self.press(spot, 1, false);
        }
        true
    }

    /// Selects the text from the first match of `from` to the end of the first match of `to`
    /// after it, in the rows' units (for shots and tests). False if they're not there.
    pub fn select_text(&self, rows: &[(SharedString, Vec<Unit>)], from: &str, to: &str) -> bool {
        let find = |needle: &str, after: Option<&Spot>, end: bool| {
            let order = self.order();
            rows.iter().flat_map(|(row, units)| units.iter().map(move |unit| (row, unit))).find_map(|(row, unit)| {
                let unit_id = UnitId { row: row.clone(), path: unit.path.clone() };
                let at = unit.text.find(needle)?;
                let spot = Spot::new(unit_id, if end { at + needle.len() } else { at });
                match after {
                    Some(after) if order.cmp(&spot, after) != Some(Ordering::Greater) => None,
                    _ => Some(spot),
                }
            })
        };
        let Some(start) = find(from, None, false) else { return false };
        let Some(end) = find(to, Some(&start), true) else { return false };
        self.select(start, end);
        true
    }
}

/// The byte index in `layout` nearest `position` (the nearer side of a character).
fn index_at(layout: &TextLayout, position: Point<Pixels>) -> Option<usize> {
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut top = bounds.top();
    let mut start = 0;
    let lines = layout.line_layouts();
    for (n, line) in lines.iter().enumerate() {
        let height = line.size(line_height).height;
        if position.y < top + height || n + 1 == lines.len() {
            let within = point(position.x - bounds.left(), (position.y - top).clamp(px(0.), height - px(0.5)));
            let ix = match line.closest_index_for_position(within, line_height) {
                Ok(ix) | Err(ix) => ix,
            };
            return Some(start + ix.min(line.len()));
        }
        top += height;
        start += line.len() + 1;
    }
    Some(0)
}

/// The rectangles covering `range` of `layout`, one for each line it takes part of. `nub`: a
/// line it runs past the end of shows a little of its newline, as a Mac's selections do.
pub fn range_rects(layout: &TextLayout, range: Range<usize>, nub: bool) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut top = bounds.top();
    let mut start = 0;
    for line in layout.line_layouts() {
        let len = line.len();
        let end = start + len;
        if range.start <= end && range.end >= start {
            let unwrapped = &line.unwrapped_layout;
            // Its rows: where each starts, as a byte index and a distance along the unwrapped line.
            let mut rows: SmallVec<[(usize, Pixels); 4]> = SmallVec::new();
            rows.push((0, px(0.)));
            for boundary in line.wrap_boundaries() {
                let glyph = &unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                rows.push((glyph.index, glyph.position.x));
            }
            let from = range.start.saturating_sub(start);
            let to = range.end.saturating_sub(start).min(len);
            let past_end = range.end > end;
            for (n, &(row_start, row_x)) in rows.iter().enumerate() {
                let (row_end, row_end_x) = rows.get(n + 1).copied().unwrap_or((len, unwrapped.width));
                let last_row = n + 1 == rows.len();
                let (a, b) = (from.max(row_start).min(row_end), to.min(row_end));
                let selected = a < b;
                // The selection takes this line's end and goes on: its newline shows.
                let newline = nub && last_row && past_end;
                if !selected && !newline {
                    continue;
                }
                let x0 = unwrapped.x_for_index(a) - row_x;
                let mut x1 = if !selected { x0 } else if b >= row_end { row_end_x - row_x } else { unwrapped.x_for_index(b) - row_x };
                if newline {
                    x1 += px(5.);
                }
                if x1 > x0 {
                    rects.push(Bounds::new(point(bounds.left() + x0, top + line_height * n as f32), size(x1 - x0, line_height)));
                }
            }
        }
        top += line.size(line_height).height;
        start = end + 1;
        if start > range.end {
            break;
        }
    }
    rects
}

type LinkClick = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Text that can be selected: a `StyledText` (with its links, if any, opening on a click), which
/// reports where it was laid out to its transcript's [`SelectionState`] and paints its part of the
/// selection under its glyphs. Inline code's fill is painted here too, as a chip under the
/// selection.
pub struct SelectableText {
    id: ElementId,
    text: SharedString,
    runs: Option<Vec<TextRun>>,
    unit: Option<(SelectionState, UnitId)>,
    chips: Vec<(Range<usize>, Hsla)>,
    links: Rc<Vec<(Range<usize>, String)>>,
    on_link: Option<LinkClick>,
    link_title: Option<Rc<dyn Fn(&str) -> String>>,
    inner: Option<(AnyElement, TextLayout)>,
}

impl SelectableText {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self { id: id.into(), text: text.into(), runs: None, unit: None, chips: Vec::new(), links: Rc::default(), on_link: None, link_title: None, inner: None }
    }

    pub fn runs(mut self, runs: Vec<TextRun>) -> Self {
        self.runs = Some(runs);
        self
    }

    /// Selectable, as `unit` of `state`'s transcript (none: drawn only).
    pub fn selectable(mut self, unit: Option<(SelectionState, UnitId)>) -> Self {
        self.unit = unit;
        self
    }

    /// Rounded fills under stretches of the text (inline code).
    pub fn chips(mut self, chips: Vec<(Range<usize>, Hsla)>) -> Self {
        self.chips = chips;
        self
    }

    /// Stretches that link, opening on a click (`open`) that didn't select anything, their
    /// address shown on hover.
    pub fn links(mut self, links: Vec<(Range<usize>, String)>, open: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.links = Rc::new(links);
        self.on_link = Some(Rc::new(open));
        self
    }

    pub fn link_title(mut self, title: impl Fn(&str) -> String + 'static) -> Self {
        self.link_title = Some(Rc::new(title));
        self
    }

    fn build(&mut self) -> &mut (AnyElement, TextLayout) {
        if self.inner.is_none() {
            let mut styled = StyledText::new(self.text.clone());
            if let Some(runs) = self.runs.take() {
                styled = styled.with_runs(runs);
            }
            let layout = styled.layout().clone();
            let element = match (&self.on_link, self.links.is_empty()) {
                (Some(open), false) => {
                    let ranges: Vec<Range<usize>> = self.links.iter().map(|(range, _)| range.clone()).collect();
                    let (links, tips) = (self.links.clone(), self.links.clone());
                    let open = open.clone();
                    let selection = self.unit.as_ref().map(|(state, _)| state.clone());
                    let text = InteractiveText::new(self.id.clone(), styled).on_click(ranges, move |ix, window, cx| {
                        // A press that selected (a drag, a double click) isn't a click on the link.
                        if selection.as_ref().is_some_and(SelectionState::press_selected) {
                            return;
                        }
                        if let Some((_, url)) = links.get(ix) {
                            open(url, window, cx);
                        }
                    });
                    // Where a link goes, shown before it's clicked.
                    let title = self.link_title.clone();
                    text.tooltip(move |ix, _, cx| tips.iter().find(|(range, _)| range.contains(&ix)).map(|(_, url)| {
                        crate::menu::Tooltip::new(title.as_ref().map_or_else(|| url.clone(), |title| title(url))).build(cx)
                    }))
                        .into_any_element()
                }
                _ => styled.into_any_element(),
            };
            self.inner = Some((element, layout));
        }
        self.inner.as_mut().expect("built")
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let (element, _) = self.build();
        (element.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Option<Hitbox> {
        // Under the links' own hitbox, so a link's pointing hand wins over the text's I-beam.
        let hitbox = self.unit.is_some().then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
        let (element, layout) = self.build();
        element.prepaint(window, cx);
        let layout = layout.clone();
        if let Some((state, unit)) = &self.unit {
            let visible = bounds.intersect(&window.content_mask().bounds);
            if visible.size.width > px(0.) && visible.size.height > px(0.) {
                state.0.borrow_mut().laid.push(Laid { unit: unit.clone(), layout, visible, text: self.text.clone(), links: self.links.clone() });
            }
        }
        hitbox
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _bounds: Bounds<Pixels>, _: &mut (), hitbox: &mut Option<Hitbox>, window: &mut Window, cx: &mut App) {
        let colors = cx.theme().colors.clone();
        let layout = self.build().1.clone();
        for (range, color) in &self.chips {
            for rect in range_rects(&layout, range.clone(), false) {
                // Inset so neighbouring lines' chips don't touch, out a hair at the sides.
                let rect = Bounds::new(point(rect.left() - px(2.), rect.top() + px(1.)), size(rect.size.width + px(4.), rect.size.height - px(2.)));
                window.paint_quad(quad(rect, px(4.), *color, px(0.), gpui::transparent_black(), Default::default()));
            }
        }
        if let Some((state, unit)) = &self.unit {
            if let Some(hitbox) = hitbox {
                window.set_cursor_style(CursorStyle::IBeam, hitbox);
            }
            let (range, emphasized) = {
                let inner = state.0.borrow();
                let range = inner.selection.as_ref().and_then(|selection| {
                    let (start, end) = match inner.order.cmp(&selection.anchor, &selection.head)? {
                        Ordering::Greater => (&selection.head, &selection.anchor),
                        _ => (&selection.anchor, &selection.head),
                    };
                    range_in(&inner.order, unit, self.text.len(), start, end).map(|range| {
                        // Runs past this unit: show a little of its end, as a newline would.
                        let past = inner.order.cmp_unit(unit, &end.unit) == Some(Ordering::Less);
                        (range, past)
                    })
                });
                (range, inner.emphasized)
            };
            if let Some((range, past)) = range {
                let color = if emphasized { colors.selection } else { colors.pressed };
                let end = if past { range.end + 1 } else { range.end };
                for rect in range_rects(&layout, range.start..end, true) {
                    window.paint_quad(gpui::fill(rect, color));
                }
            }
        }
        if let Some((element, _)) = self.inner.as_mut() {
            element.paint(window, cx);
        }
    }
}

type Scroll = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

/// The area a transcript's selection is made in: it takes presses, drags and releases in it, and
/// while a drag is held past its top or bottom, scrolls (`scroll`) and keeps selecting.
pub struct SelectionArea {
    state: SelectionState,
    focus: FocusHandle,
    child: AnyElement,
    scroll: Option<Scroll>,
    in_use: Option<bool>,
}

impl SelectionArea {
    /// `focus`: what takes the keyboard when a press selects, so ⌘C and ⌘A come to the transcript.
    pub fn new(state: SelectionState, focus: FocusHandle, child: impl IntoElement) -> Self {
        Self { state, focus, child: child.into_any_element(), scroll: None, in_use: None }
    }

    /// Shows the selection as in use, or set aside, whatever has the keyboard (for the gallery).
    pub fn shown_in_use(mut self, in_use: bool) -> Self {
        self.in_use = Some(in_use);
        self
    }

    /// Scrolls the transcript by a distance (negative: back in time).
    pub fn on_scroll(mut self, scroll: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.scroll = Some(Rc::new(scroll));
        self
    }
}

/// How far past an edge a held drag is (negative: above the top), if it is.
fn past_edge(bounds: &Bounds<Pixels>, pointer: Point<Pixels>) -> Option<Pixels> {
    if pointer.y < bounds.top() {
        Some(pointer.y - bounds.top())
    } else if pointer.y > bounds.bottom() {
        Some(pointer.y - bounds.bottom())
    } else {
        None
    }
}

/// The spot to select to while the pointer's out of `bounds`: the nearest point inside.
fn inside(bounds: &Bounds<Pixels>, pointer: Point<Pixels>) -> Point<Pixels> {
    point(pointer.x.clamp(bounds.left() + px(1.), bounds.right() - px(1.)), pointer.y.clamp(bounds.top() + px(1.), bounds.bottom() - px(1.)))
}

impl Element for SelectionArea {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Hitbox {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // A drag held past an edge scrolls, faster the further out it is.
        let (pending, pointer) = {
            let inner = self.state.0.borrow();
            (inner.pending && inner.moved && inner.deferred.is_none(), inner.pointer)
        };
        let past = pointer.filter(|_| pending).and_then(|pointer| past_edge(&bounds, pointer).map(|past| (pointer, past)));
        if let (Some((_, past)), Some(scroll)) = (past, &self.scroll) {
            let speed = (past.abs() / 3.).clamp(px(2.), px(60.));
            scroll(if past < px(0.) { -speed } else { speed }, window, cx);
        }
        {
            let mut inner = self.state.0.borrow_mut();
            inner.laid.clear();
            inner.emphasized = self.in_use.unwrap_or_else(|| self.focus.contains_focused(window, cx) || crate::menu::is_menu_focused(window, cx));
        }
        self.child.prepaint(window, cx);
        if let Some((pointer, _)) = past {
            if let Some(spot) = self.state.spot_at(inside(&bounds, pointer)) {
                self.state.drag_to(spot);
            }
            window.request_animation_frame();
        }
        hitbox
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), hitbox: &mut Hitbox, window: &mut Window, cx: &mut App) {
        // Listened to before what's inside is painted, so a press it takes (a button's) comes to
        // it first, and the selection's handlers run after.
        let state = self.state.clone();
        let focus = self.focus.clone();
        let area = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !area.is_hovered(window) {
                return;
            }
            let Some(spot) = state.spot_at(event.position) else { return };
            let on_text = state.0.borrow().laid.iter().any(|laid| laid.visible.contains(&event.position));
            {
                let mut inner = state.0.borrow_mut();
                inner.origin = Some(event.position);
                inner.pointer = Some(event.position);
                inner.moved = false;
                inner.deferred = None;
            }
            if on_text || event.modifiers.shift {
                // On text, a click places it, two take a word, three a line; the keyboard comes to
                // the transcript at once (⌘A, ⌘C).
                state.press(spot, if on_text { event.click_count.max(1) } else { 1 }, event.modifiers.shift);
                window.focus(&focus, cx);
            } else {
                // Off the text (a step's row, a button, the margin): only a drag selects, so a click
                // or double-click there leaves the selection and the composer's keyboard alone.
                state.press_off_text(spot);
                // Nor does the transcript (or the conversation around it) take the keyboard for it.
                window.prevent_default();
            }
            window.refresh();
        });
        let state = self.state.clone();
        let focus = self.focus.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !state.is_dragging() {
                return;
            }
            if event.pressed_button != Some(MouseButton::Left) {
                // The release happened where we didn't hear it.
                state.release();
                window.refresh();
                return;
            }
            if !state.drag_begins(event.position) {
                return;
            }
            if let Some(spot) = state.spot_at(inside(&bounds, event.position)) {
                state.drag_to(spot);
            }
            if !state.is_empty() && !focus.contains_focused(window, cx) {
                window.focus(&focus, cx);
            }
            window.refresh();
        });
        let state = self.state.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, _| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !state.is_dragging() {
                return;
            }
            state.release();
            window.refresh();
        });
        self.child.paint(window, cx);
        // The card over a finished selection was placed from the frame before: if its end has moved
        // (scrolled, or the window resized) or gone, draw again.
        let (finished, placed, used) = (self.state.is_finished(), self.state.0.borrow().card_at, self.state.0.borrow().card_used);
        if used && finished && self.state.end_bounds() != placed {
            window.refresh();
        }
    }
}

/// Writes `text` to the clipboard, if there's any.
pub fn copy_to_clipboard(text: String, cx: &mut App) {
    if !text.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(row: &str, path: &[u32], offset: usize) -> Spot {
        Spot::new(UnitId::new(row.to_owned(), path), offset)
    }

    #[test]
    fn spots_order_by_row_then_reading_order_then_offset() {
        let order = Order::new(["m1", "m2"]);
        assert_eq!(order.cmp(&spot("m1", &[3, 1], 9), &spot("m2", &[0], 0)), Some(Ordering::Less));
        // A nested block comes after its list's earlier items and before the next.
        assert_eq!(order.cmp(&spot("m1", &[1, 0, 0], 4), &spot("m1", &[1, 1, 0], 0)), Some(Ordering::Less));
        assert_eq!(order.cmp(&spot("m1", &[2], 4), &spot("m1", &[2], 1)), Some(Ordering::Greater));
        // A row's ends hold everything in it.
        assert_eq!(order.cmp(&Spot::row_start("m1"), &spot("m1", &[0], 0)), Some(Ordering::Less));
        assert_eq!(order.cmp(&Spot::row_end("m1"), &spot("m1", &[99, 9], 1_000)), Some(Ordering::Greater));
        // A row that's gone has no place.
        assert_eq!(order.cmp(&spot("gone", &[0], 0), &spot("m1", &[0], 0)), None);
    }

    #[test]
    fn a_unit_knows_its_part_of_a_selection() {
        let order = Order::new(["m1", "m2"]);
        let (start, end) = (spot("m1", &[1], 4), spot("m2", &[0], 3));
        let unit = |row: &str, path: &[u32]| UnitId::new(row.to_owned(), path);
        assert_eq!(range_in(&order, &unit("m1", &[0]), 10, &start, &end), None, "before it");
        assert_eq!(range_in(&order, &unit("m1", &[1]), 10, &start, &end), Some(4..10), "where it starts");
        assert_eq!(range_in(&order, &unit("m1", &[2, 0]), 6, &start, &end), Some(0..6), "between");
        assert_eq!(range_in(&order, &unit("m2", &[0]), 10, &start, &end), Some(0..3), "where it ends");
        assert_eq!(range_in(&order, &unit("m2", &[1]), 10, &start, &end), None, "after it");
        // An end past what's there (text that's since shrunk) is held to it.
        assert_eq!(range_in(&order, &unit("m1", &[1]), 3, &start, &end), None);
    }

    #[test]
    fn words_and_lines_are_found_around_a_place() {
        let text = "Run `cargo test`, then ship it.";
        assert_eq!(&text[word_around(text, 6)], "cargo");
        assert_eq!(&text[word_around(text, 3)], " ");
        assert_eq!(&text[word_around(text, text.len())], ".");
        let accented = "café déjà vu";
        assert_eq!(&accented[word_around(accented, 7)], "déjà");
        let code = "fn main() {\n    run();\n}";
        assert_eq!(&code[line_around(code, 15)], "    run();");
        assert_eq!(&code[line_around(code, 0)], "fn main() {");
    }

    fn reply_units() -> Vec<Unit> {
        vec![
            Unit::new(&[0, 0], "The fix:", Lead::Paragraph),
            Unit::new(&[0, 1, 0, 0], "Keep what was typed.", Lead::Item { depth: 0, marker: "- ".into() }),
            Unit::new(&[0, 1, 1, 0], "Show the error.", Lead::Item { depth: 0, marker: "- ".into() }),
            Unit::new(&[0, 1, 1, 1, 0, 0], "under the field", Lead::Item { depth: 1, marker: "1. ".into() }),
            Unit::new(&[0, 2], "fn save() {\n    post();\n}", Lead::Paragraph),
            Unit::new(&[0, 3, 0, 0], "File", Lead::Paragraph),
            Unit::new(&[0, 3, 0, 1], "Change", Lead::Cell),
            Unit::new(&[0, 3, 1, 0], "Form.tsx", Lead::Line),
            Unit::new(&[0, 3, 1, 1], "+17", Lead::Cell),
        ]
    }

    #[test]
    fn copying_a_whole_reply_keeps_its_shape_in_plain_text() {
        let order = Order::new(["m1", "m2"]);
        let rows = vec![("m1".into(), vec![Unit::new(&[0], "Fix the form", Lead::Paragraph)]), ("m2".into(), reply_units())];
        let text = copy_text(&order, rows, &Spot::row_start("m1"), &Spot::row_end("m2"));
        assert_eq!(text, "Fix the form\n\nThe fix:\n\n- Keep what was typed.\n- Show the error.\n  1. under the field\n\nfn save() {\n    post();\n}\n\nFile\tChange\nForm.tsx\t+17");
    }

    #[test]
    fn copying_part_of_a_reply_takes_only_whats_selected() {
        let order = Order::new(["m2"]);
        let rows = || vec![("m2".into(), reply_units())];
        // From inside one item into the code: the first item has no marker, the next keeps its.
        let text = copy_text(&order, rows(), &spot("m2", &[0, 1, 0, 0], 5), &spot("m2", &[0, 2], 11));
        assert_eq!(text, "what was typed.\n- Show the error.\n  1. under the field\n\nfn save() {");
        // Inside the code only.
        let text = copy_text(&order, rows(), &spot("m2", &[0, 2], 16), &spot("m2", &[0, 2], 23));
        assert_eq!(text, "post();");
        // A selection starting at an item's very start keeps its marker.
        let text = copy_text(&order, rows(), &spot("m2", &[0, 1, 1, 0], 0), &spot("m2", &[0, 1, 1, 0], 4));
        assert_eq!(text, "- Show");
    }

    #[test]
    fn copying_across_messages_joins_them_with_a_blank_line_and_skips_rows_without_text() {
        let order = Order::new(["m1", "turn", "m2"]);
        let rows = vec![
            ("m1".into(), vec![Unit::new(&[0], "Fix the form", Lead::Paragraph)]),
            ("turn".into(), Vec::new()),
            ("m2".into(), vec![Unit::new(&[0, 0], "Done. Both pass.", Lead::Paragraph)]),
        ];
        let text = copy_text(&order, rows, &spot("m1", &[0], 4), &spot("m2", &[0, 0], 5));
        assert_eq!(text, "the form\n\nDone.");
    }

    #[test]
    fn a_double_click_drag_extends_by_words_and_select_all_widens_to_everything() {
        let state = SelectionState::new();
        state.set_rows(["m1", "m2"]);
        state.select(spot("m1", &[0], 0), spot("m1", &[0], 0));
        // Without layout (no frame drawn), a press still places a spot; words come from laid text,
        // so here a double click is the empty word.
        state.press(spot("m1", &[0], 2), 1, false);
        state.drag_to(spot("m2", &[0], 3));
        assert_eq!(state.range(), Some((spot("m1", &[0], 2), spot("m2", &[0], 3))));
        assert!(state.press_selected());
        state.release();
        // ⌘A: the row last pressed in (where the selection's head is), then everything.
        state.select_all("m1", "m2");
        assert_eq!(state.range(), Some((Spot::row_start("m2"), Spot::row_end("m2"))));
        state.select_all("m1", "m2");
        assert_eq!(state.range(), Some((Spot::row_start("m1"), Spot::row_end("m2"))));
        // A click that selects nothing leaves nothing selected.
        state.press(spot("m1", &[0], 1), 1, false);
        state.release();
        assert!(state.is_empty());
        // Rows going away take the selection with them.
        state.select(spot("m1", &[0], 0), spot("m2", &[0], 3));
        state.set_rows(["m2"]);
        assert!(state.is_empty());
    }

    #[test]
    fn a_shift_click_extends_from_where_the_selection_began() {
        let state = SelectionState::new();
        state.set_rows(["m1"]);
        state.press(spot("m1", &[1], 4), 1, false);
        state.drag_to(spot("m1", &[1], 9));
        state.release();
        state.press(spot("m1", &[0], 2), 1, true);
        state.release();
        assert_eq!(state.range(), Some((spot("m1", &[0], 2), spot("m1", &[1], 4))));
    }
}
