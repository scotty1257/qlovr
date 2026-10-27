//! The log surface: gutter, rows, highlights and the two thin scrollbars, drawn on one canvas.
//!
//! Only the visible rows are ever decoded or drawn, so scrolling a multi-GB file costs the same as
//! scrolling an empty one.
//!
//! iced draws through `&self`, so the canvas cannot move the view itself. Mouse input becomes a
//! [`ViewMessage`] the app applies with `&mut`; the only thing written back during a draw is the
//! viewport size, which the keyboard paging in `update` needs and only layout knows.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastlogviewer_core::{
    copy_text_of, join_texts, ColumnShape, FilterIndex, Joined, LogFile, LogTemplate, LogViewMode,
    MatchIndex, Matcher, TagDef, MAX_LINE_BYTES,
};
use iced::alignment::Vertical;
use iced::keyboard::{self, key::Named, Key};
use iced::mouse;
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Path, Stroke, Text};
use iced::widget::text::{Alignment, Shaping};
use iced::{Color, Pixels, Point, Rectangle, Renderer, Size, Theme};

use crate::format::thousands;
use crate::theme;

pub const ROW_H: f32 = 20.0;
/// JetBrains Mono advances every glyph 600/1000 em. Highlight rectangles are placed at
/// `column * CHAR_W`, so this has to be the font's real advance, not a measured approximation.
pub const CHAR_W: f32 = theme::MONO_SIZE * 0.6;
const SCROLLBAR: f32 = 12.0;
const GUTTER_PAD: f32 = 14.0;
const TEXT_GAP: f32 = 12.0;
const MIN_THUMB: f32 = 32.0;
/// Rows per wheel notch.
const WHEEL_ROWS: f32 = 3.0;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Mouse input from the canvas, applied by the app.
#[derive(Debug, Clone)]
pub enum ViewMessage {
    ScrollRows(i64),
    ScrollCols(i64),
    ScrollTo(i64),
    ScrollColTo(i64),
    Select(usize),
    /// Double-click on a row: zooms into a pattern, or wraps/unwraps a log line.
    Activate(usize),
    /// A press-and-drag over text: where it started and where the pointer is now.
    DragText(TextPos, TextPos),
    /// A click on a row's note marker, or on the "+" that adds one.
    NoteMarker(usize),
}

/// Width of the strip at the gutter's left edge where note markers sit.
const MARKER_W: f32 = 18.0;

/// A character position: display row and char column within that row's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub display: usize,
    pub col: usize,
}

/// What a key press did that the app has to act on.
#[derive(Default)]
pub struct KeyOutcome {
    pub template_activated: bool,
    pub escape_pressed: bool,
    pub copy_requested: bool,
}

pub struct LogView {
    top: usize,
    left_col: usize,
    /// -1 when nothing is selected, which is "before the first row" for keyboard navigation.
    selected: i64,
    /// The other end of a multi-line selection (Shift+click, Shift+arrows); -1 for none.
    anchor: i64,
    /// Characters selected by dragging, as (where the drag began, where it is now).
    text_sel: Option<(TextPos, TextPos)>,

    mode: LogViewMode,
    templates: Vec<Arc<LogTemplate>>,
    rare_threshold: usize,

    file: Option<Arc<LogFile>>,
    matcher: Option<Arc<Matcher>>,
    matches: Option<Arc<MatchIndex>>,
    active_filter: Option<Arc<dyn FilterIndex>>,
    pub current_match_line: i64,
    tags: Vec<TagDef>,

    /// Column layout lines are drawn in, when Columns is on and the file has one.
    columns: Option<Arc<ColumnShape>>,

    /// Real lines with a line note.
    note_lines: HashSet<i64>,
    /// Patterns with a note; their lines get a pattern marker.
    note_patterns: Vec<Arc<LogTemplate>>,

    /// Wrap every line to the view's width.
    wrap_all: bool,
    /// Real line numbers wrapped one at a time (Enter or double-click).
    wrapped: HashSet<i64>,

    /// Text area (canvas minus scrollbars), as of the last draw.
    viewport: Cell<Size>,
    /// The display row drawn on each screen row, and which wrapped piece of it, as of the last
    /// draw. Wrapped lines span several screen rows, so mouse hit-testing needs this instead of
    /// `top + y / ROW_H`.
    row_lines: RefCell<Vec<(usize, usize)>>,
}

impl LogView {
    pub fn new() -> Self {
        LogView {
            top: 0,
            left_col: 0,
            selected: -1,
            anchor: -1,
            text_sel: None,
            mode: LogViewMode::Lines,
            templates: Vec::new(),
            rare_threshold: 0,
            file: None,
            matcher: None,
            matches: None,
            active_filter: None,
            current_match_line: -1,
            tags: Vec::new(),
            columns: None,
            note_lines: HashSet::new(),
            note_patterns: Vec::new(),
            wrap_all: false,
            wrapped: HashSet::new(),
            viewport: Cell::new(Size::ZERO),
            row_lines: RefCell::new(Vec::new()),
        }
    }

    // ------------------------------------------------------------------ state

    pub fn mode(&self) -> LogViewMode {
        self.mode
    }

    pub fn filter_on(&self) -> bool {
        self.active_filter.is_some()
    }

    pub fn active_filter(&self) -> Option<Arc<dyn FilterIndex>> {
        self.active_filter.clone()
    }

    pub fn visible_rows(&self) -> usize {
        (((self.viewport.get().height - self.header_h()) / ROW_H) as usize).max(1)
    }

    pub fn set_columns(&mut self, columns: Option<Arc<ColumnShape>>) {
        self.columns = columns;
        self.scroll_cols_to(self.left_col as i64);
        self.scroll_to(self.top as i64);
    }

    /// Height of the column header row: there is one only while columns are shown.
    fn header_h(&self) -> f32 {
        if self.columns.is_some() && self.mode == LogViewMode::Lines {
            ROW_H
        } else {
            0.0
        }
    }

    /// A line as drawn: laid out in columns when it fits the column layout, otherwise as is.
    fn shown_text(&self, raw: String) -> String {
        match &self.columns {
            Some(columns) if self.mode == LogViewMode::Lines => columns
                .render(raw.trim_end_matches(['\r', '\n']))
                .unwrap_or(raw),
            _ => raw,
        }
    }

    fn visible_cols(&self) -> usize {
        (((self.viewport.get().width - self.text_x()) / CHAR_W) as usize).max(1)
    }

    pub fn display_count(&self) -> usize {
        if self.mode == LogViewMode::Templates {
            return self.templates.len();
        }
        let Some(file) = &self.file else { return 0 };
        match &self.active_filter {
            Some(f) => f.count(),
            None => file.line_count(),
        }
    }

    fn actual_line(&self, display: usize) -> i64 {
        match &self.active_filter {
            Some(f) => f.get(display) as i64,
            None => display as i64,
        }
    }

    pub fn selected_actual_line(&self) -> i64 {
        if self.selected >= 0 && (self.selected as usize) < self.display_count() {
            self.actual_line(self.selected as usize)
        } else {
            -1
        }
    }

    pub fn selected_template(&self) -> Option<Arc<LogTemplate>> {
        if self.mode == LogViewMode::Templates && self.selected >= 0 {
            self.templates.get(self.selected as usize).cloned()
        } else {
            None
        }
    }

    pub fn wrap_all(&self) -> bool {
        self.wrap_all
    }

    pub fn toggle_wrap_all(&mut self) {
        self.wrap_all = !self.wrap_all;
        self.wrapped.clear();
        self.scroll_cols_to(self.left_col as i64);
        self.scroll_to(self.top as i64);
    }

    /// Wraps or unwraps one line, by display row. Does nothing while every line is wrapped.
    pub fn toggle_wrap_line(&mut self, display: usize) {
        if self.mode != LogViewMode::Lines || self.wrap_all || display >= self.display_count() {
            return;
        }
        let line = self.actual_line(display);
        if !self.wrapped.remove(&line) {
            self.wrapped.insert(line);
        }
        self.scroll_to(self.top as i64);
    }

    fn is_wrapped(&self, line: i64) -> bool {
        self.mode == LogViewMode::Lines && (self.wrap_all || self.wrapped.contains(&line))
    }

    /// Screen rows a display row takes up.
    fn line_rows(&self, display: usize) -> usize {
        let line = self.actual_line(display);
        if !self.is_wrapped(line) {
            return 1;
        }
        let Some(file) = &self.file else { return 1 };
        let chars = self
            .shown_text(file.get_line(line as usize, MAX_LINE_BYTES))
            .chars()
            .count();
        chars.div_ceil(self.visible_cols()).max(1)
    }

    pub fn set_file(&mut self, file: Option<Arc<LogFile>>) {
        self.file = file;
        self.top = 0;
        self.left_col = 0;
        self.selected = -1;
        self.clear_extra_selection();
        self.wrapped.clear();
        self.matches = None;
        self.current_match_line = -1;
    }

    pub fn set_matches(&mut self, matcher: Option<Arc<Matcher>>, matches: Option<Arc<MatchIndex>>) {
        let keep = self.selected_actual_line();
        self.matcher = matcher;
        self.matches = matches;
        self.current_match_line = -1;
        if keep >= 0 {
            self.go_to_actual(keep);
        }
    }

    pub fn set_tags(&mut self, tags: Vec<TagDef>) {
        self.tags = tags;
    }

    pub fn set_note_marks(&mut self, lines: HashSet<i64>, patterns: Vec<Arc<LogTemplate>>) {
        self.note_lines = lines;
        self.note_patterns = patterns;
    }

    /// Whether a real line has a line note and whether it belongs to a pattern with a note.
    fn note_marks(&self, line: i64) -> (bool, bool) {
        let in_pattern = self.note_patterns.iter().any(|t| {
            let i = t.index_at_or_after(line as i32);
            i < t.count() && t.get(i) as i64 == line
        });
        (self.note_lines.contains(&line), in_pattern)
    }

    pub fn set_active_filter(&mut self, filter: Option<Arc<dyn FilterIndex>>) {
        let keep = self.selected_actual_line();
        self.active_filter = filter;
        self.top = 0;
        self.selected = -1;
        self.clear_extra_selection();
        if keep >= 0 {
            self.go_to_actual(keep);
        }
    }

    /// Switch to the pattern list, or refresh it in place while the build streams in.
    pub fn show_templates(&mut self, templates: Vec<Arc<LogTemplate>>, rare_threshold: usize) {
        self.templates = templates;
        self.rare_threshold = rare_threshold;
        if self.mode != LogViewMode::Templates {
            self.mode = LogViewMode::Templates;
            self.top = 0;
            self.left_col = 0;
            self.selected = -1;
            self.clear_extra_selection();
        }
        if self.selected >= self.templates.len() as i64 {
            self.selected = self.templates.len() as i64 - 1;
        }
        self.scroll_to(self.top as i64);
    }

    pub fn show_lines(&mut self) {
        if self.mode == LogViewMode::Lines {
            return;
        }
        self.mode = LogViewMode::Lines;
        self.top = 0;
        self.selected = -1;
        self.clear_extra_selection();
    }

    // -------------------------------------------------------------- selection

    /// Drops the multi-line and text selections, leaving at most the one selected row.
    fn clear_extra_selection(&mut self) {
        self.anchor = -1;
        self.text_sel = None;
    }

    /// Extends the line selection from its anchor to `display` (Shift+click, Shift+arrows).
    pub fn extend_selection(&mut self, display: usize) {
        if self.mode != LogViewMode::Lines || display >= self.display_count() {
            return;
        }
        if self.anchor < 0 {
            self.anchor = if self.selected >= 0 { self.selected } else { display as i64 };
        }
        self.selected = display as i64;
        self.text_sel = None;
    }

    pub fn select_all(&mut self) {
        let count = self.display_count();
        if self.mode != LogViewMode::Lines || count == 0 {
            return;
        }
        self.anchor = 0;
        self.selected = count as i64 - 1;
        self.text_sel = None;
    }

    /// Selects characters from `from` to `to`; the rows they span become the line selection.
    pub fn set_text_selection(&mut self, from: TextPos, to: TextPos) {
        let count = self.display_count();
        if self.mode != LogViewMode::Lines || from.display >= count || to.display >= count {
            return;
        }
        self.anchor = from.display as i64;
        self.selected = to.display as i64;
        self.text_sel = Some((from, to));
    }

    /// The selected display rows, first to last. `None` with nothing selected or outside lines.
    pub fn selected_line_range(&self) -> Option<(usize, usize)> {
        let count = self.display_count() as i64;
        if self.mode != LogViewMode::Lines || self.selected < 0 || self.selected >= count {
            return None;
        }
        let anchor = if (0..count).contains(&self.anchor) {
            self.anchor
        } else {
            self.selected
        };
        Some((
            self.selected.min(anchor) as usize,
            self.selected.max(anchor) as usize,
        ))
    }

    /// The dragged-over characters in reading order, or `None` when nothing (or nothing but an
    /// empty span) is selected.
    fn text_range(&self) -> Option<(TextPos, TextPos)> {
        let (a, b) = self.text_sel?;
        let count = self.display_count();
        if a == b || self.mode != LogViewMode::Lines || a.display >= count || b.display >= count {
            return None;
        }
        Some((a.min(b), a.max(b)))
    }

    pub fn has_text_selection(&self) -> bool {
        self.text_range().is_some()
    }

    /// The dragged-over text, one piece per row, capped at `limit` bytes.
    pub fn selected_text(&self, limit: usize) -> Option<Joined> {
        let (a, b) = self.text_range()?;
        let file = self.file.clone()?;
        let pieces = (a.display..=b.display).map(move |display| {
            let chars: Vec<char> = self
                .shown_text(copy_text_of(&file, self.actual_line(display) as usize))
                .chars()
                .collect();
            let from = if display == a.display { a.col } else { 0 }.min(chars.len());
            let to = if display == b.display { b.col } else { chars.len() }.clamp(from, chars.len());
            chars[from..to].iter().collect::<String>()
        });
        Some(join_texts(pieces, limit))
    }

    /// Scrolls just enough to show `display`.
    fn reveal(&mut self, display: i64) {
        let rows = self.visible_rows() as i64;
        if display < self.top as i64 {
            self.scroll_to(display);
        } else if display >= self.top as i64 + rows {
            self.scroll_to(display - rows + 1);
        }
    }

    // --------------------------------------------------------------- scrolling

    fn max_top(&self) -> usize {
        let count = self.display_count();
        let rows = self.visible_rows();
        let wrapping = self.wrap_all || !self.wrapped.is_empty();
        if self.mode == LogViewMode::Templates || !wrapping {
            return count.saturating_sub(rows);
        }
        // Walk back from the end until the last lines fill a screen.
        let mut top = count;
        let mut used = 0;
        while top > 0 {
            let height = self.line_rows(top - 1);
            if used + height > rows {
                break;
            }
            used += height;
            top -= 1;
        }
        top.min(count.saturating_sub(1))
    }

    fn max_left(&self) -> usize {
        if self.mode == LogViewMode::Templates || self.wrap_all {
            return 0;
        }
        let longest = self.file.as_ref().map_or(0, |f| f.max_line_bytes().max(0) as usize);
        (longest + 1).saturating_sub(self.visible_cols())
    }

    pub fn scroll_by(&mut self, delta: i64) {
        self.scroll_to(self.top as i64 + delta);
    }

    pub fn scroll_to(&mut self, display: i64) {
        self.top = display.clamp(0, self.max_top() as i64) as usize;
    }

    pub fn scroll_cols_by(&mut self, delta: i64) {
        self.scroll_cols_to(self.left_col as i64 + delta);
    }

    pub fn scroll_cols_to(&mut self, col: i64) {
        self.left_col = col.clamp(0, self.max_left() as i64) as usize;
    }

    pub fn go_to_display(&mut self, display: i64) {
        let count = self.display_count();
        if count == 0 {
            return;
        }
        let display = display.clamp(0, count as i64 - 1);
        self.selected = display;
        self.clear_extra_selection();
        let rows = self.visible_rows() as i64;
        if display < self.top as i64 || display >= self.top as i64 + rows {
            self.scroll_to(display - rows / 3);
        }
    }

    /// Navigate by real file line number, filtered or not.
    pub fn go_to_actual(&mut self, actual_line: i64) {
        if self.file.is_none() {
            return;
        }
        let display = match &self.active_filter {
            Some(f) => {
                let mut i = f.index_at_or_after(actual_line as i32);
                if i >= f.count() {
                    i = f.count().saturating_sub(1);
                }
                i as i64
            }
            None => actual_line,
        };
        self.go_to_display(display);
    }

    /// Selects the first row and scrolls to it (Ctrl+Home, or the toolbar's Top button).
    pub fn go_to_start(&mut self) {
        if self.mode == LogViewMode::Templates {
            self.move_template_selection(i64::MIN / 2);
        } else {
            self.go_to_display(0);
        }
    }

    /// Selects the last row and scrolls to it (Ctrl+End, or the toolbar's Bottom button).
    pub fn go_to_end(&mut self) {
        if self.mode == LogViewMode::Templates {
            self.move_template_selection(i64::MAX / 2);
        } else {
            self.go_to_display(self.display_count() as i64 - 1);
        }
    }

    /// The display row the status bar reports: the selection, or the top row when nothing is
    /// selected. `None` when there are no rows.
    pub fn current_display(&self) -> Option<usize> {
        let count = self.display_count();
        if count == 0 {
            return None;
        }
        if self.selected >= 0 && (self.selected as usize) < count {
            Some(self.selected as usize)
        } else {
            Some(self.top.min(count - 1))
        }
    }

    /// The real file line at the top of the view, or -1 when there are no rows.
    pub fn top_line(&self) -> i64 {
        if self.mode == LogViewMode::Lines && self.top < self.display_count() {
            self.actual_line(self.top)
        } else {
            -1
        }
    }

    /// The real file line shown on display row `display`.
    pub fn line_at(&self, display: usize) -> i64 {
        self.actual_line(display)
    }

    pub fn select(&mut self, display: usize) {
        if display < self.display_count() {
            self.selected = display as i64;
            self.clear_extra_selection();
        }
    }

    fn move_template_selection(&mut self, delta: i64) {
        if self.templates.is_empty() {
            return;
        }
        // Unselected is -1, i.e. "before the first row", so the first Down lands on 0 and the
        // first End still lands on the last row.
        let target = self.selected.saturating_add(delta);
        self.selected = target.clamp(0, self.templates.len() as i64 - 1);

        let rows = self.visible_rows() as i64;
        if self.selected < self.top as i64 {
            self.scroll_to(self.selected);
        } else if self.selected >= self.top as i64 + rows {
            self.scroll_to(self.selected - rows + 1);
        }
    }

    fn gutter_w(&self) -> f32 {
        if self.mode == LogViewMode::Templates {
            return 11.0 * CHAR_W + 2.0 * GUTTER_PAD; // room for a grouped count like "1,204,882"
        }
        let Some(file) = &self.file else { return 0.0 };
        let digits = file.line_count().max(1).ilog10() as f32 + 1.0;
        digits.max(3.0) * CHAR_W + 2.0 * GUTTER_PAD
    }

    fn text_x(&self) -> f32 {
        self.gutter_w() + TEXT_GAP
    }

    // ------------------------------------------------------------------ input

    /// Keys the view owns. The app forwards these only when no text input consumed them.
    pub fn handle_key(&mut self, key: &Key, modifiers: keyboard::Modifiers) -> KeyOutcome {
        let mut outcome = KeyOutcome::default();
        let rows = self.visible_rows() as i64;

        if self.mode == LogViewMode::Templates {
            match key {
                Key::Named(Named::ArrowUp) => self.move_template_selection(-1),
                Key::Named(Named::ArrowDown) => self.move_template_selection(1),
                Key::Named(Named::PageUp) => self.move_template_selection(-rows),
                Key::Named(Named::PageDown) => self.move_template_selection(rows),
                Key::Named(Named::Home) => self.move_template_selection(i64::MIN / 2),
                Key::Named(Named::End) => self.move_template_selection(i64::MAX / 2),
                Key::Named(Named::Enter) => {
                    outcome.template_activated = self.selected_template().is_some()
                }
                Key::Named(Named::Escape) => outcome.escape_pressed = true,
                _ => {}
            }
            return outcome;
        }

        if self.file.is_none() {
            return outcome;
        }
        let ctrl = modifiers.command();
        let shift = modifiers.shift();

        match key {
            Key::Named(Named::ArrowUp) if shift && self.selected > 0 => {
                self.extend_selection(self.selected as usize - 1);
                self.reveal(self.selected);
            }
            Key::Named(Named::ArrowDown) if shift && self.selected >= 0 => {
                self.extend_selection(self.selected as usize + 1);
                self.reveal(self.selected);
            }
            Key::Named(Named::ArrowUp) => self.scroll_by(-1),
            Key::Named(Named::ArrowDown) => self.scroll_by(1),
            Key::Named(Named::PageUp) => self.scroll_by(-rows),
            Key::Named(Named::PageDown) => self.scroll_by(rows),
            Key::Named(Named::ArrowLeft) => self.scroll_cols_to(self.left_col as i64 - 8),
            Key::Named(Named::ArrowRight) => self.scroll_cols_to(self.left_col as i64 + 8),
            Key::Named(Named::Home) if ctrl => self.go_to_start(),
            Key::Named(Named::Home) => self.left_col = 0,
            Key::Named(Named::End) if ctrl => self.go_to_end(),
            Key::Named(Named::End) => self.scroll_cols_to(i64::MAX),
            Key::Named(Named::Enter) if self.selected >= 0 => {
                self.toggle_wrap_line(self.selected as usize)
            }
            Key::Named(Named::Escape) => outcome.escape_pressed = true,
            Key::Character(c) if ctrl && c.as_str().eq_ignore_ascii_case("c") => {
                outcome.copy_requested = true;
            }
            Key::Character(c) if ctrl && c.as_str().eq_ignore_ascii_case("a") => self.select_all(),
            _ => {}
        }
        outcome
    }

    // ------------------------------------------------------------------ paint

    fn paint_lines(&self, frame: &mut Frame, hover_row: Option<usize>) {
        let file = self.file.clone().expect("checked by the caller");
        let size = self.viewport.get();
        let gutter_w = self.gutter_w();
        let text_x = self.text_x();

        frame.fill_rectangle(Point::ORIGIN, Size::new(gutter_w, size.height), theme::GUTTER);
        frame.fill_rectangle(
            Point::new(gutter_w - 1.0, 0.0),
            Size::new(1.0, size.height),
            theme::BORDER,
        );

        let count = self.display_count();
        let cols = self.visible_cols() + 2;
        let wrap_cols = self.visible_cols();
        let screen_rows = self.visible_rows() + 1; // one extra so a partial last row is drawn
        let highlight = self.matcher.as_ref().is_some_and(|m| !m.is_empty() && m.is_valid());
        let mut ranges: Vec<i32> = Vec::with_capacity(64);
        let mut tag_ranges: Vec<i32> = Vec::with_capacity(32);
        let mut ink: Vec<Color> = Vec::new();
        let line_sel = self.selected_line_range();
        let text_sel = self.text_range();
        let mut row_lines = self.row_lines.borrow_mut();
        row_lines.clear();

        let mut display = self.top;
        while display < count && row_lines.len() < screen_rows {
            let line = self.actual_line(display);
            let full = self.shown_text(file.get_line(line as usize, MAX_LINE_BYTES));
            let chars: Vec<char> = full.chars().collect();
            let y = self.header_h() + row_lines.len() as f32 * ROW_H;
            let is_current = line == self.current_match_line;
            let is_selected = line_sel.is_some_and(|(a, b)| (a..=b).contains(&display));
            let level = level_span(&full);

            // A wrapped line is cut into `wrap_cols` pieces from column 0; an unwrapped one is a
            // single piece starting at the horizontal scroll position.
            let (first_col, piece_len, pieces) = if self.is_wrapped(line) {
                (0, wrap_cols, chars.len().div_ceil(wrap_cols).max(1))
            } else {
                (self.left_col, cols, 1)
            };
            let pieces = pieces.min(screen_rows - row_lines.len());
            let height = pieces as f32 * ROW_H;
            row_lines.extend((0..pieces).map(|piece| (display, piece)));

            if is_selected {
                frame.fill_rectangle(
                    Point::new(gutter_w, y),
                    Size::new(size.width - gutter_w, height),
                    theme::SEL_BG,
                );
            } else if hover_row == Some(display) {
                frame.fill_rectangle(
                    Point::new(gutter_w, y),
                    Size::new(size.width - gutter_w, height),
                    theme::HOVER_ROW,
                );
            }

            // A level-coloured tick at the gutter's edge makes errors findable while scrolling.
            if let Some((_, _, color)) = level.filter(|l| l.2 == theme::ERROR || l.2 == theme::WARN)
            {
                frame.fill_rectangle(Point::new(0.0, y + 2.0), Size::new(3.0, height - 4.0), color);
            }
            if is_selected {
                frame.fill_rectangle(Point::new(gutter_w, y), Size::new(2.0, height), theme::ACCENT);
            }

            // Notes: a dot for a line note, a ring for a pattern note, a faint "+" on hover to
            // add one.
            let (line_note, pattern_note) = self.note_marks(line);
            let center = Point::new(MARKER_W / 2.0 + 1.0, y + ROW_H / 2.0);
            if line_note {
                frame.fill(&Path::circle(center, 3.5), theme::ACCENT);
            }
            if pattern_note {
                let radius = if line_note { 6.0 } else { 4.0 };
                frame.stroke(
                    &Path::circle(center, radius),
                    Stroke::default().with_color(theme::WARN).with_width(1.5),
                );
            }
            if !line_note && !pattern_note && hover_row == Some(display) {
                frame.fill_text(Text {
                    content: "+".to_string(),
                    position: center,
                    color: theme::TEXT_FAINT,
                    size: Pixels(theme::MONO_SIZE),
                    font: theme::MONO,
                    align_x: Alignment::Center,
                    align_y: Vertical::Center,
                    shaping: Shaping::Basic,
                    ..Text::default()
                });
            }

            frame.fill_text(Text {
                content: (line + 1).to_string(),
                position: Point::new(gutter_w - GUTTER_PAD, y + ROW_H / 2.0),
                color: if is_selected { theme::TEXT_DIM } else { theme::TEXT_FAINT },
                size: Pixels(theme::MONO_SIZE),
                font: theme::MONO,
                align_x: Alignment::Right,
                align_y: Vertical::Center,
                shaping: Shaping::Basic,
                ..Text::default()
            });

            ranges.clear();
            if highlight {
                if let Some(m) = &self.matcher {
                    m.append_ranges(&full, &mut ranges);
                }
            }

            // Colour every character once: timestamp, level word, matches, plain text.
            ink.clear();
            ink.resize(chars.len(), theme::TEXT);
            let mut paint = |start: i32, len: i32, color: Color| {
                let a = start.max(0) as usize;
                let b = ((start + len).max(0) as usize).min(ink.len());
                if a < b {
                    ink[a..b].fill(color);
                }
            };
            if let Some(len) = timestamp_len(&full) {
                paint(0, len as i32, theme::TEXT_DIM);
            }
            if let Some((start, len, color)) = level {
                paint(start as i32, len as i32, color);
            }
            let match_fg = if is_current { theme::CUR_FG } else { theme::MATCH_FG };
            for pair in ranges.chunks_exact(2) {
                paint(pair[0], pair[1], match_fg);
            }
            let match_bg = if is_current { theme::CUR_BG } else { theme::MATCH_BG };

            for piece in 0..pieces {
                let y = y + piece as f32 * ROW_H;
                let start = first_col + piece * piece_len;

                // 0) tag highlight backgrounds, behind the ad-hoc search highlight
                for tag in &self.tags {
                    if !tag.highlight_on {
                        continue;
                    }
                    let Some(matcher) = &tag.matcher else { continue };
                    if matcher.is_empty() || !matcher.is_valid() {
                        continue;
                    }
                    tag_ranges.clear();
                    matcher.append_ranges(&full, &mut tag_ranges);
                    let [r8, g8, b8] = tag.color();
                    let brush = Color::from_rgba8(r8, g8, b8, 0.30);
                    for pair in tag_ranges.chunks_exact(2) {
                        self.fill_columns(frame, pair, start, piece_len, y, brush);
                    }
                }

                // 1) match backgrounds
                for pair in ranges.chunks_exact(2) {
                    self.fill_columns(frame, pair, start, piece_len, y, match_bg);
                }

                // 1b) dragged-over text
                if let Some((a, b)) =
                    text_sel.filter(|(a, b)| (a.display..=b.display).contains(&display))
                {
                    let from = if display == a.display { a.col } else { 0 };
                    let to = if display == b.display { b.col } else { chars.len() };
                    if to > from {
                        let pair = [from as i32, (to - from) as i32];
                        let brush = theme::alpha(theme::ACCENT, 0.35);
                        self.fill_columns(frame, &pair, start, piece_len, y, brush);
                    }
                }

                // 2) the text, split into runs of one colour each so nothing is drawn twice
                let end = (start + piece_len).min(chars.len());
                let mut run_start = start;
                for i in start + 1..=end {
                    if i == end || ink[i] != ink[run_start] {
                        frame.fill_text(Text {
                            content: chars[run_start..i].iter().collect(),
                            position: Point::new(
                                text_x + (run_start - start) as f32 * CHAR_W,
                                y + ROW_H / 2.0,
                            ),
                            color: ink[run_start],
                            size: Pixels(theme::MONO_SIZE),
                            font: theme::MONO,
                            align_y: Vertical::Center,
                            shaping: Shaping::Basic,
                            ..Text::default()
                        });
                        run_start = i;
                    }
                }
            }
            display += 1;
        }
        drop(row_lines);
        self.paint_column_header(frame);
    }

    /// The pinned row naming each column, scrolled sideways with the text.
    fn paint_column_header(&self, frame: &mut Frame) {
        let Some(columns) = self.columns.as_ref().filter(|_| self.header_h() > 0.0) else {
            return;
        };
        let size = self.viewport.get();
        let gutter_w = self.gutter_w();
        frame.fill_rectangle(Point::ORIGIN, Size::new(size.width, ROW_H), theme::GUTTER);
        frame.fill_rectangle(
            Point::new(0.0, ROW_H - 1.0),
            Size::new(size.width, 1.0),
            theme::BORDER,
        );
        frame.fill_rectangle(
            Point::new(gutter_w - 1.0, 0.0),
            Size::new(1.0, ROW_H),
            theme::BORDER,
        );
        for (header, offset) in columns.headers.iter().zip(columns.offsets()) {
            if offset < self.left_col {
                continue;
            }
            frame.fill_text(Text {
                content: header.to_uppercase(),
                position: Point::new(
                    self.text_x() + (offset - self.left_col) as f32 * CHAR_W,
                    ROW_H / 2.0,
                ),
                color: theme::TEXT_FAINT,
                size: Pixels(theme::SMALL_SIZE),
                font: theme::UI,
                align_y: Vertical::Center,
                ..Text::default()
            });
        }
    }

    /// Fills the background behind character range `pair` = `[start, len]`, clipped to the
    /// `len` columns starting at column `first`.
    fn fill_columns(
        &self,
        frame: &mut Frame,
        pair: &[i32],
        first: usize,
        len: usize,
        y: f32,
        color: Color,
    ) {
        let c0 = (pair[0] - first as i32).max(0);
        let c1 = (pair[0] + pair[1] - first as i32).min(len as i32);
        if c1 <= c0 {
            return;
        }
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(self.text_x() + c0 as f32 * CHAR_W - 1.0, y + 1.0),
                Size::new((c1 - c0) as f32 * CHAR_W + 2.0, ROW_H - 2.0),
                3.0.into(),
            ),
            color,
        );
    }

    /// The zoomed-out view: one row per pattern, its line count in the gutter over a bar showing
    /// its share of the busiest pattern; rare patterns starred and drawn in the accent colour.
    fn paint_templates(&self, frame: &mut Frame, hover_row: Option<usize>) {
        let size = self.viewport.get();
        if self.templates.is_empty() {
            frame.fill_text(Text {
                content: "Clustering…".to_string(),
                position: Point::new(size.width / 2.0, size.height / 2.0),
                color: theme::TEXT_FAINT,
                size: Pixels(theme::UI_SIZE),
                font: theme::UI,
                align_x: Alignment::Center,
                align_y: Vertical::Center,
                ..Text::default()
            });
            return;
        }

        let gutter_w = self.gutter_w();
        let text_x = self.text_x();
        let cols = self.visible_cols();
        let busiest = self.templates.iter().map(|t| t.count()).max().unwrap_or(1).max(1);

        frame.fill_rectangle(Point::ORIGIN, Size::new(gutter_w, size.height), theme::GUTTER);
        frame.fill_rectangle(
            Point::new(gutter_w - 1.0, 0.0),
            Size::new(1.0, size.height),
            theme::BORDER,
        );

        for r in 0..=self.visible_rows() {
            let i = self.top + r;
            let Some(template) = self.templates.get(i) else {
                break;
            };

            let y = r as f32 * ROW_H;
            let rare = template.count() <= self.rare_threshold;
            let is_selected = i as i64 == self.selected;

            if is_selected {
                frame.fill_rectangle(
                    Point::new(gutter_w, y),
                    Size::new(size.width - gutter_w, ROW_H),
                    theme::SEL_BG,
                );
                frame.fill_rectangle(Point::new(gutter_w, y), Size::new(2.0, ROW_H), theme::ACCENT);
            } else if hover_row == Some(i) {
                frame.fill_rectangle(
                    Point::new(gutter_w, y),
                    Size::new(size.width - gutter_w, ROW_H),
                    theme::HOVER_ROW,
                );
            }

            let share = template.count() as f32 / busiest as f32;
            let bar_w = ((gutter_w - 8.0) * share).max(2.0);
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(gutter_w - 4.0 - bar_w, y + 3.0),
                    Size::new(bar_w, ROW_H - 6.0),
                    3.0.into(),
                ),
                theme::alpha(if rare { theme::ACCENT } else { theme::TEXT_DIM }, 0.12),
            );

            frame.fill_text(Text {
                content: thousands(template.count() as u64),
                position: Point::new(gutter_w - GUTTER_PAD, y + ROW_H / 2.0),
                color: if rare { theme::ACCENT } else { theme::TEXT_DIM },
                size: Pixels(theme::MONO_SIZE),
                font: theme::MONO,
                align_x: Alignment::Right,
                align_y: Vertical::Center,
                shaping: Shaping::Basic,
                ..Text::default()
            });

            let rendered = template.render();
            let text = if rare { format!("★ {rendered}") } else { rendered };
            frame.fill_text(Text {
                content: text.chars().take(cols).collect(),
                position: Point::new(text_x, y + ROW_H / 2.0),
                color: if rare { theme::ACCENT } else { theme::TEXT },
                size: Pixels(theme::MONO_SIZE),
                font: theme::MONO,
                align_y: Vertical::Center,
                ..Text::default()
            });
        }
    }

    fn paint_scrollbars(&self, frame: &mut Frame, bounds: Size, state: &CanvasState) {
        let size = self.viewport.get();
        let count = self.display_count();

        let v_track = Rectangle::new(Point::new(size.width, 0.0), Size::new(SCROLLBAR, size.height));
        frame.fill_rectangle(v_track.position(), v_track.size(), theme::BG);
        frame.fill_rectangle(
            Point::new(size.width, 0.0),
            Size::new(1.0, size.height),
            theme::alpha(theme::BORDER, 0.6),
        );

        // Ticks only make sense unfiltered — filtered, every row is a match.
        if let (Some(marks), None, LogViewMode::Lines) =
            (&self.matches, &self.active_filter, self.mode)
        {
            let total = self.file.as_ref().map_or(0, |f| f.line_count());
            let n = marks.count();
            if n > 0 && total > 0 {
                let step = (n / 1200).max(1);
                let h = (size.height - 2.0).max(1.0);
                let mut i = 0;
                while i < n {
                    let y = marks.get(i) as f32 * h / total as f32;
                    frame.fill_rectangle(
                        Point::new(v_track.x + 3.0, y),
                        Size::new(SCROLLBAR - 6.0, 2.0),
                        theme::TICK,
                    );
                    i += step;
                }
            }
        }

        if let Some(thumb) = v_thumb(count, self.visible_rows(), self.max_top(), self.top, v_track)
        {
            let hot = state.drag.is_some_and(|d| d.vertical) || state.hot == Some(true);
            fill_thumb(frame, thumb, hot, true);
        }

        let h_track = Rectangle::new(
            Point::new(0.0, size.height),
            Size::new(size.width, bounds.height - size.height),
        );
        frame.fill_rectangle(h_track.position(), h_track.size(), theme::BG);
        let cols = self.visible_cols();
        let h_track_text = Rectangle {
            x: self.text_x(),
            width: (h_track.width - self.text_x()).max(0.0),
            ..h_track
        };
        if let Some(thumb) = h_thumb(self.max_left(), cols, self.left_col, h_track_text) {
            let hot = state.drag.is_some_and(|d| !d.vertical) || state.hot == Some(false);
            fill_thumb(frame, thumb, hot, false);
        }
    }

    /// The display row under `point`; `usize::MAX` below the last line.
    fn row_at(&self, point: Point) -> usize {
        if point.y < self.header_h() {
            return usize::MAX;
        }
        let screen_row = ((point.y - self.header_h()) / ROW_H).max(0.0) as usize;
        if self.mode == LogViewMode::Templates {
            return self.top + screen_row;
        }
        self.row_lines.borrow().get(screen_row).map_or(usize::MAX, |r| r.0)
    }

    /// The character position nearest `point`, clamped to the drawn rows so a drag past the top
    /// or bottom edge still lands somewhere. `None` outside lines mode or with nothing drawn.
    fn text_pos_at(&self, point: Point) -> Option<TextPos> {
        if self.mode != LogViewMode::Lines {
            return None;
        }
        let row_lines = self.row_lines.borrow();
        let last = row_lines.len().checked_sub(1)?;
        let screen_row = (((point.y - self.header_h()) / ROW_H).max(0.0) as usize).min(last);
        let (display, piece) = row_lines[screen_row];
        let col = ((point.x - self.text_x()) / CHAR_W).round().max(0.0) as usize;
        let col = if self.is_wrapped(self.actual_line(display)) {
            piece * self.visible_cols() + col
        } else {
            self.left_col + col
        };
        Some(TextPos { display, col })
    }
}

fn fill_thumb(frame: &mut Frame, thumb: Rectangle, hot: bool, vertical: bool) {
    let inset = if hot { 2.0 } else { 3.0 };
    let rect = if vertical {
        Rectangle { x: thumb.x + inset, width: thumb.width - inset * 2.0, ..thumb }
    } else {
        Rectangle { y: thumb.y + inset, height: thumb.height - inset * 2.0, ..thumb }
    };
    frame.fill(
        &Path::rounded_rectangle(rect.position(), rect.size(), 999.0.into()),
        if hot { theme::TEXT_DIM } else { theme::BORDER_STRONG },
    );
}

/// Thumb geometry for a vertical track, or `None` when everything fits.
fn v_thumb(
    count: usize,
    rows: usize,
    max_top: usize,
    top: usize,
    track: Rectangle,
) -> Option<Rectangle> {
    if max_top == 0 || track.height <= 0.0 {
        return None;
    }
    let len = (track.height * rows as f32 / count as f32).clamp(MIN_THUMB.min(track.height), track.height);
    let pos = (track.height - len) * top as f32 / max_top as f32;
    Some(Rectangle::new(Point::new(track.x, track.y + pos), Size::new(track.width, len)))
}

fn h_thumb(max_left: usize, cols: usize, left: usize, track: Rectangle) -> Option<Rectangle> {
    if max_left == 0 || track.width <= 0.0 {
        return None;
    }
    let total = max_left + cols;
    let len = (track.width * cols as f32 / total as f32).clamp(MIN_THUMB.min(track.width), track.width);
    let pos = (track.width - len) * left as f32 / max_left as f32;
    Some(Rectangle::new(Point::new(track.x + pos, track.y), Size::new(len, track.height)))
}

/// Length in chars of a leading timestamp — a first token that starts with a digit and holds a
/// `:` — so it can be drawn dimmer than the message after it.
fn timestamp_len(line: &str) -> Option<usize> {
    let token = line.split([' ', '\t']).next()?;
    let looks_like_time = token.starts_with(|c: char| c.is_ascii_digit()) && token.contains(':');
    looks_like_time.then(|| token.chars().count())
}

/// Finds a log level word (`ERROR`, `WARN`, …) near the start of a line: its char offset, char
/// length and colour. Upper-case only, and only within the first 96 characters, so a message that
/// merely *mentions* "info" further along is left alone.
fn level_span(line: &str) -> Option<(usize, usize, Color)> {
    let mut start: Option<usize> = None;
    let mut word = String::new();
    for (i, c) in line.chars().take(96).chain(std::iter::once(' ')).enumerate() {
        if c.is_ascii_uppercase() {
            if start.is_none() {
                start = Some(i);
                word.clear();
            }
            word.push(c);
            continue;
        }
        if c.is_alphanumeric() {
            start = None; // part of a longer mixed-case token
            continue;
        }
        if let Some(s) = start.take() {
            let color = match word.as_str() {
                "ERROR" | "ERR" | "FATAL" | "CRITICAL" | "CRIT" => theme::ERROR,
                "WARN" | "WARNING" => theme::WARN,
                "INFO" => theme::INFO,
                "DEBUG" | "DBG" | "TRACE" => theme::DEBUG,
                _ => continue,
            };
            return Some((s, word.len(), color));
        }
    }
    None
}

// ------------------------------------------------------------------- canvas

#[derive(Clone, Copy)]
pub struct Drag {
    vertical: bool,
    /// Pointer offset from the thumb's leading edge when the drag began.
    grab: f32,
}

#[derive(Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    /// Which scrollbar the pointer is over: `Some(true)` vertical, `Some(false)` horizontal.
    hot: Option<bool>,
    hover_row: Option<usize>,
    /// When and on which row the last click landed, to recognise a double-click.
    last_click: Option<(Instant, usize)>,
    /// Where the left button went down over text, while it is held: the start of a text drag.
    press: Option<TextPos>,
}

impl canvas::Program<ViewMessage> for LogView {
    type State = CanvasState;

    fn update(
        &self,
        state: &mut CanvasState,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<ViewMessage>> {
        let size = Size::new(
            (bounds.width - SCROLLBAR).max(0.0),
            (bounds.height - SCROLLBAR).max(0.0),
        );
        let v_track = Rectangle::new(Point::new(size.width, 0.0), Size::new(SCROLLBAR, size.height));
        let h_track = Rectangle::new(
            Point::new(self.text_x(), size.height),
            Size::new((size.width - self.text_x()).max(0.0), SCROLLBAR),
        );
        let count = self.display_count();
        let rows = self.visible_rows();
        let max_top = self.max_top();
        let content = Rectangle::new(Point::ORIGIN, size);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = cursor.position_in(bounds)?;
                if v_track.contains(p) {
                    let thumb = v_thumb(count, rows, max_top, self.top, v_track)?;
                    if thumb.contains(p) {
                        state.drag = Some(Drag { vertical: true, grab: p.y - thumb.y });
                        return Some(Action::request_redraw().and_capture());
                    }
                    // Clicking the track outside the thumb pages by one screenful.
                    let page = if p.y < thumb.y { -(rows as i64) } else { rows as i64 };
                    return Some(Action::publish(ViewMessage::ScrollRows(page)).and_capture());
                }
                if h_track.contains(p) {
                    let cols = self.visible_cols();
                    let thumb = h_thumb(self.max_left(), cols, self.left_col, h_track)?;
                    if thumb.contains(p) {
                        state.drag = Some(Drag { vertical: false, grab: p.x - thumb.x });
                        return Some(Action::request_redraw().and_capture());
                    }
                    let page = if p.x < thumb.x { -(cols as i64) } else { cols as i64 };
                    return Some(Action::publish(ViewMessage::ScrollCols(page)).and_capture());
                }
                if !content.contains(p) {
                    return None;
                }

                let row = self.row_at(p);
                if row >= count {
                    return None;
                }
                if self.mode == LogViewMode::Lines && p.x < MARKER_W {
                    state.press = None;
                    return Some(Action::publish(ViewMessage::NoteMarker(row)).and_capture());
                }
                let now = Instant::now();
                let double = state
                    .last_click
                    .is_some_and(|(at, last_row)| last_row == row && now - at < DOUBLE_CLICK);
                state.last_click = if double { None } else { Some((now, row)) };
                state.press = if double { None } else { self.text_pos_at(p) };
                let message = if double {
                    ViewMessage::Activate(row)
                } else {
                    ViewMessage::Select(row)
                };
                Some(Action::publish(message).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.press = None;
                state.drag.take()?;
                Some(Action::request_redraw())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(drag) = state.drag {
                    let p = cursor.position_from(bounds.position())?;
                    return if drag.vertical {
                        let thumb = v_thumb(count, rows, max_top, self.top, v_track)?;
                        let span = v_track.height - thumb.height;
                        let at = ((p.y - drag.grab) / span).clamp(0.0, 1.0);
                        let top = (at * max_top as f32).round() as i64;
                        Some(Action::publish(ViewMessage::ScrollTo(top)))
                    } else {
                        let max_left = self.max_left();
                        let thumb = h_thumb(max_left, self.visible_cols(), self.left_col, h_track)?;
                        let span = h_track.width - thumb.width;
                        let at = ((p.x - h_track.x - drag.grab) / span).clamp(0.0, 1.0);
                        let col = (at * max_left as f32).round() as i64;
                        Some(Action::publish(ViewMessage::ScrollColTo(col)))
                    };
                }
                if let Some(from) = state.press {
                    let p = cursor.position_from(bounds.position())?;
                    let to = self.text_pos_at(p)?;
                    return (to != from || self.text_sel.is_some())
                        .then(|| Action::publish(ViewMessage::DragText(from, to)));
                }

                let p = cursor.position_in(bounds);
                let hot = p.and_then(|p| {
                    if v_track.contains(p) {
                        Some(true)
                    } else if h_track.contains(p) {
                        Some(false)
                    } else {
                        None
                    }
                });
                let hover_row = p
                    .filter(|p| content.contains(*p))
                    .map(|p| self.row_at(p))
                    .filter(|row| *row < count);
                if hot != state.hot || hover_row != state.hover_row {
                    state.hot = hot;
                    state.hover_row = hover_row;
                    return Some(Action::request_redraw());
                }
                None
            }
            Event::Mouse(mouse::Event::CursorLeft) => {
                state.hot = None;
                state.hover_row = None;
                Some(Action::request_redraw())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                cursor.position_in(bounds)?;
                let (x, y) = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => (x * WHEEL_ROWS, y * WHEEL_ROWS),
                    mouse::ScrollDelta::Pixels { x, y } => (x / CHAR_W, y / ROW_H),
                };
                let message = if x.abs() > y.abs() {
                    ViewMessage::ScrollCols(-x.round() as i64)
                } else {
                    ViewMessage::ScrollRows(-y.round() as i64)
                };
                // The hover highlight follows the content under a still pointer.
                state.hover_row = None;
                Some(Action::publish(message).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &CanvasState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        self.viewport.set(Size::new(
            (bounds.width - SCROLLBAR).max(0.0),
            (bounds.height - SCROLLBAR).max(0.0),
        ));

        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG);

        if self.mode == LogViewMode::Templates {
            self.paint_templates(&mut frame, state.hover_row);
        } else if self.file.is_some() {
            self.paint_lines(&mut frame, state.hover_row);
        }
        self.paint_scrollbars(&mut frame, bounds.size(), state);

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &CanvasState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            return mouse::Interaction::Grabbing;
        }
        let size = self.viewport.get();
        let over_marker = cursor
            .position_in(bounds)
            .is_some_and(|p| p.x < MARKER_W && self.row_at(p) < self.display_count());
        if self.mode == LogViewMode::Lines && over_marker && state.press.is_none() {
            return mouse::Interaction::Pointer;
        }
        let over_text = cursor.position_in(bounds).is_some_and(|p| {
            p.x >= self.text_x() && p.x < size.width && p.y < size.height
        });
        if self.mode == LogViewMode::Lines && (over_text || state.press.is_some()) {
            return mouse::Interaction::Text;
        }
        if self.mode == LogViewMode::Templates
            && cursor.position_in(bounds).is_some_and(|p| self.row_at(p) < self.templates.len())
            && state.hot.is_none()
        {
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::default()
    }
}
