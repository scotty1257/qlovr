//! The log surface: gutter, rows, highlights and the two thin scrollbars.
//!
//! Only the visible rows are ever decoded or drawn — roughly 60 `Painter::text` calls a frame — so
//! scrolling a multi-GB file costs the same as scrolling an empty one.
//!
//! The C# control raised a `SelectionChanged` event so the form could refresh its status line.
//! egui recomputes the whole status line every frame anyway, so only the two events that actually
//! *change behaviour* survive as a return value.

use std::sync::Arc;

use eframe::egui::{pos2, vec2, Align2, Color32, Context, FontId, Id, Key, Rect, Sense, Ui};
use fastlogviewer_core::{
    FilterIndex, LogFile, LogTemplate, LogViewMode, MatchIndex, Matcher, TagDef, MAX_LINE_BYTES,
};

use crate::theme;
use crate::widgets::{thousands, ThinScrollBar};

#[derive(Default)]
pub struct ViewEvents {
    pub template_activated: bool,
    pub escape_pressed: bool,
}

pub struct LogView {
    vs: ThinScrollBar,
    hs: ThinScrollBar,

    top: usize,
    left_col: usize,
    row_h: f32,
    text_h: f32,
    char_w: f32,
    gutter_w: f32,
    /// -1 when nothing is selected, which is "before the first row" for keyboard navigation.
    selected: i64,

    mode: LogViewMode,
    templates: Vec<Arc<LogTemplate>>,
    rare_threshold: usize,

    file: Option<Arc<LogFile>>,
    matcher: Option<Arc<Matcher>>,
    matches: Option<Arc<MatchIndex>>,
    active_filter: Option<Arc<dyn FilterIndex>>,
    pub current_match_line: i64,
    tags: Vec<TagDef>,

    ranges: Vec<i32>,
    tag_ranges: Vec<i32>,

    view_w: f32,
    view_h: f32,
    font: FontId,
}

impl LogView {
    pub fn new() -> Self {
        LogView {
            vs: ThinScrollBar::new(true),
            hs: ThinScrollBar::new(false),
            top: 0,
            left_col: 0,
            row_h: 16.0,
            text_h: 13.0,
            char_w: 7.0,
            gutter_w: 0.0,
            selected: -1,
            mode: LogViewMode::Lines,
            templates: Vec::new(),
            rare_threshold: 0,
            file: None,
            matcher: None,
            matches: None,
            active_filter: None,
            current_match_line: -1,
            tags: Vec::new(),
            ranges: Vec::with_capacity(64),
            tag_ranges: Vec::with_capacity(32),
            view_w: 0.0,
            view_h: 0.0,
            font: theme::mono(10.0),
        }
    }

    // ------------------------------------------------------------------ state

    pub fn mode(&self) -> LogViewMode {
        self.mode
    }

    pub fn filter_on(&self) -> bool {
        self.active_filter.is_some()
    }

    fn filtering(&self) -> bool {
        self.active_filter.is_some()
    }

    pub fn visible_rows(&self) -> usize {
        ((self.view_h / self.row_h) as usize).max(1)
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

    pub fn set_file(&mut self, file: Option<Arc<LogFile>>) {
        self.file = file;
        self.top = 0;
        self.left_col = 0;
        self.selected = -1;
        self.matches = None;
        self.current_match_line = -1;
        self.sync_scroll_bars();
    }

    pub fn set_matches(&mut self, matcher: Option<Arc<Matcher>>, matches: Option<Arc<MatchIndex>>) {
        let keep = self.selected_actual_line();
        self.matcher = matcher;
        self.matches = matches;
        self.current_match_line = -1;
        if keep >= 0 {
            self.go_to_actual(keep);
        } else {
            self.sync_scroll_bars();
        }
    }

    pub fn set_tags(&mut self, tags: Vec<TagDef>) {
        self.tags = tags;
    }

    pub fn set_active_filter(&mut self, filter: Option<Arc<dyn FilterIndex>>) {
        let keep = self.selected_actual_line();
        self.active_filter = filter;
        self.top = 0;
        self.selected = -1;
        self.sync_scroll_bars();
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
            self.selected = -1;
        }
        if self.selected >= self.templates.len() as i64 {
            self.selected = self.templates.len() as i64 - 1;
        }
        self.sync_scroll_bars();
    }

    pub fn show_lines(&mut self) {
        if self.mode == LogViewMode::Lines {
            return;
        }
        self.mode = LogViewMode::Lines;
        self.top = 0;
        self.selected = -1;
        self.sync_scroll_bars();
    }

    // --------------------------------------------------------------- scrolling

    pub fn scroll_by(&mut self, delta: i64) {
        self.scroll_to(self.top as i64 + delta);
    }

    pub fn scroll_to(&mut self, display: i64) {
        let max = self.display_count().saturating_sub(self.visible_rows()) as i64;
        self.top = display.clamp(0, max.max(0)) as usize;
        self.vs.set_value(self.top as i32);
    }

    pub fn go_to_display(&mut self, display: i64) {
        let count = self.display_count();
        if count == 0 {
            return;
        }
        let display = display.clamp(0, count as i64 - 1);
        self.selected = display;
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

    fn text_x_offset(&self) -> f32 {
        self.gutter_w + 10.0
    }

    pub fn sync_scroll_bars(&mut self) {
        if self.mode == LogViewMode::Templates {
            self.gutter_w = 12.0 * self.char_w; // room for a grouped count like "1,204,882"
            self.vs.set_large_change(self.visible_rows() as i32);
            self.vs.set_small_change(1);
            self.vs.set_maximum(self.templates.len() as i32 - 1);
            let max_top = self.templates.len().saturating_sub(self.visible_rows());
            if self.top > max_top {
                self.top = max_top;
            }
            self.vs.set_value(self.top as i32);
            self.hs.set_maximum(0);
            self.hs.set_value(0);
            self.left_col = 0;
            return;
        }

        let lines = self.display_count();
        let rows = self.visible_rows();

        let mut max_line_no = self.file.as_ref().map_or(0, |f| f.line_count());
        let mut digits = 1;
        while max_line_no >= 10 {
            max_line_no /= 10;
            digits += 1;
        }
        self.gutter_w = if self.file.is_none() {
            0.0
        } else {
            digits as f32 * self.char_w + 20.0
        };

        self.vs.set_large_change(rows as i32);
        self.vs.set_small_change(1);
        self.vs.set_maximum(lines as i32 - 1);
        let max_top = lines.saturating_sub(rows);
        if self.top > max_top {
            self.top = max_top;
        }
        self.vs.set_value(self.top as i32);

        let cols = (((self.view_w - self.text_x_offset()) / self.char_w) as i32).max(1);
        self.hs.set_large_change(cols);
        self.hs.set_small_change(1);
        self.hs
            .set_maximum(self.file.as_ref().map_or(0, |f| f.max_line_bytes().max(0)));
        if self.left_col as i32 > self.hs.maximum() {
            self.left_col = self.hs.maximum() as usize;
        }
        self.hs.set_value(self.left_col as i32);
    }

    /// Measures a ten-character run and divides, exactly as the C# `MeasureFont` did. A single
    /// `glyph_width` call is *not* equivalent: egui rounds each glyph's advance during layout, so a
    /// per-glyph width drifts away from a laid-out string and the highlight overlays — which are
    /// positioned as `column * char_w` — slide off the text they are meant to cover.
    fn measure_font(&mut self, ctx: &Context) {
        let size = ctx.fonts(|f| {
            f.layout_no_wrap(
                "0000000000".to_string(),
                self.font.clone(),
                eframe::egui::Color32::WHITE,
            )
            .size()
        });
        self.char_w = (size.x / 10.0).max(1.0);
        self.text_h = size.y.max(1.0);
        self.row_h = self.text_h + 4.0; // a little leading; dense text is hard to scan
    }

    // ------------------------------------------------------------------ input

    /// Keys the view owns. The caller forwards these only when no text editor holds focus, which is
    /// how WinForms split `ProcessCmdKey` from `LogView.OnKeyDown`.
    pub fn handle_keys(&mut self, ctx: &Context) -> ViewEvents {
        let mut events = ViewEvents::default();

        if self.mode == LogViewMode::Templates {
            let rows = self.visible_rows() as i64;
            ctx.input_mut(|i| {
                if i.key_pressed(Key::ArrowUp) {
                    self.move_template_selection(-1);
                } else if i.key_pressed(Key::ArrowDown) {
                    self.move_template_selection(1);
                } else if i.key_pressed(Key::PageUp) {
                    self.move_template_selection(-rows);
                } else if i.key_pressed(Key::PageDown) {
                    self.move_template_selection(rows);
                } else if i.key_pressed(Key::Home) {
                    self.move_template_selection(i64::MIN / 2);
                } else if i.key_pressed(Key::End) {
                    self.move_template_selection(i64::MAX / 2);
                } else if i.key_pressed(Key::Enter) {
                    events.template_activated = true;
                } else if i.key_pressed(Key::Escape) {
                    events.escape_pressed = true;
                }
            });
            if events.template_activated && self.selected_template().is_none() {
                events.template_activated = false;
            }
            return events;
        }

        if self.file.is_none() {
            return events;
        }

        let rows = self.visible_rows() as i64;
        let h_max = self.hs.max_value();
        let mut copy: Option<i64> = None;

        ctx.input_mut(|i| {
            let ctrl = i.modifiers.command || i.modifiers.ctrl;
            if i.key_pressed(Key::ArrowUp) {
                self.scroll_by(-1);
            } else if i.key_pressed(Key::ArrowDown) {
                self.scroll_by(1);
            } else if i.key_pressed(Key::PageUp) {
                self.scroll_by(-rows);
            } else if i.key_pressed(Key::PageDown) {
                self.scroll_by(rows);
            } else if i.key_pressed(Key::ArrowLeft) {
                let v = (self.hs.value() - 8).max(0);
                self.hs.set_value(v);
                self.left_col = v as usize;
            } else if i.key_pressed(Key::ArrowRight) {
                let v = (self.hs.value() + 8).min(h_max);
                self.hs.set_value(v);
                self.left_col = v as usize;
            } else if i.key_pressed(Key::Home) {
                if ctrl {
                    self.scroll_to(0);
                } else {
                    self.hs.set_value(0);
                    self.left_col = 0;
                }
            } else if i.key_pressed(Key::End) {
                if ctrl {
                    self.scroll_to(self.display_count() as i64);
                } else {
                    self.hs.set_value(h_max);
                    self.left_col = h_max as usize;
                }
            } else if ctrl && i.key_pressed(Key::C) {
                if self.selected >= 0 {
                    copy = Some(self.actual_line(self.selected as usize));
                }
            } else if i.key_pressed(Key::Escape) {
                events.escape_pressed = true;
            }
        });

        if let (Some(line), Some(file)) = (copy, self.file.clone()) {
            let text = file.get_line(line as usize, 1 << 20);
            ctx.output_mut(|o| {
                o.copied_text = if text.is_empty() {
                    " ".to_string()
                } else {
                    text
                }
            });
        }

        events
    }

    // ------------------------------------------------------------------ paint

    pub fn ui(&mut self, ui: &mut Ui) -> ViewEvents {
        let mut events = ViewEvents::default();
        let rect = ui.available_rect_before_wrap();

        self.measure_font(ui.ctx());
        self.view_w = (rect.width() - theme::SCROLLBAR).max(0.0);
        self.view_h = (rect.height() - theme::SCROLLBAR).max(0.0);
        self.sync_scroll_bars();

        let content = Rect::from_min_size(rect.min, vec2(self.view_w, self.view_h));
        let response = ui.interact(content, Id::new("logview_surface"), Sense::click());

        // --- mouse ---
        if let Some(pointer) = response.interact_pointer_pos() {
            if response.clicked() || response.double_clicked() {
                let row = self.top as i64 + ((pointer.y - content.top()) / self.row_h) as i64;
                if self.mode == LogViewMode::Templates {
                    if row < self.templates.len() as i64 {
                        self.selected = row;
                        if response.double_clicked() {
                            events.template_activated = true;
                        }
                    }
                } else if self.file.is_some() && row < self.display_count() as i64 {
                    self.selected = row;
                }
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.scroll_by(-(scroll / self.row_h).round() as i64);
            }
        }

        // --- paint ---
        let painter = ui.painter_at(content);
        painter.rect_filled(content, 0.0, theme::BG);

        if self.mode == LogViewMode::Templates {
            self.paint_templates(&painter, content);
        } else if self.file.is_none() {
            self.paint_empty_state(&painter, content);
        } else {
            self.paint_lines(&painter, content);
        }

        // --- scrollbars ---
        // Ticks only make sense unfiltered — filtered, every row is a match.
        let marks = if self.filtering() {
            None
        } else {
            self.matches.clone()
        };
        let mark_total = self.file.as_ref().map_or(0, |f| f.line_count() as i32);

        let vs_rect = Rect::from_min_size(
            pos2(rect.right() - theme::SCROLLBAR, rect.top()),
            vec2(theme::SCROLLBAR, self.view_h),
        );
        let hs_rect = Rect::from_min_size(
            pos2(rect.left(), rect.bottom() - theme::SCROLLBAR),
            vec2(self.view_w, theme::SCROLLBAR),
        );

        if self
            .vs
            .ui(ui, vs_rect, Id::new("logview_vs"), marks.as_deref(), mark_total)
        {
            self.top = self.vs.value() as usize;
        }
        if self.hs.ui(ui, hs_rect, Id::new("logview_hs"), None, 0) {
            self.left_col = self.hs.value() as usize;
        }

        events
    }

    fn paint_lines(&mut self, painter: &eframe::egui::Painter, content: Rect) {
        let file = self.file.clone().expect("checked by the caller");

        let text_x = content.left() + self.text_x_offset();
        if self.gutter_w > 0.0 {
            painter.rect_filled(
                Rect::from_min_size(content.min, vec2(self.gutter_w, self.view_h)),
                0.0,
                theme::GUTTER,
            );
            theme::hairline(
                painter,
                theme::BORDER,
                pos2(content.left() + self.gutter_w, content.top()),
                pos2(content.left() + self.gutter_w, content.bottom()),
            );
        }

        let count = self.display_count();
        let rows = self.visible_rows();
        let cols = (((self.view_w - self.text_x_offset()) / self.char_w) as usize).max(1) + 2;
        let highlight = self
            .matcher
            .as_ref()
            .is_some_and(|m| !m.is_empty() && m.is_valid());
        let pad = (self.row_h - self.text_h) / 2.0;

        for r in 0..rows {
            let display = self.top + r;
            if display >= count {
                break;
            }

            let line = self.actual_line(display);
            let full = file.get_line(line as usize, MAX_LINE_BYTES);
            let y = content.top() + r as f32 * self.row_h;
            let ty = y + pad;
            let is_current = line == self.current_match_line;
            let is_selected = display as i64 == self.selected;

            if is_selected {
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(content.left() + self.gutter_w + 1.0, y),
                        vec2(self.view_w - self.gutter_w - 1.0, self.row_h),
                    ),
                    0.0,
                    theme::SEL_BG,
                );
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(content.left() + self.gutter_w + 1.0, y),
                        vec2(2.0, self.row_h),
                    ),
                    0.0,
                    theme::ACCENT,
                );
            }

            if self.gutter_w > 0.0 {
                painter.text(
                    pos2(content.left() + self.gutter_w - 10.0, ty),
                    Align2::RIGHT_TOP,
                    (line + 1).to_string(),
                    self.font.clone(),
                    if is_selected {
                        theme::TEXT_DIM
                    } else {
                        theme::TEXT_FAINT
                    },
                );
            }

            self.ranges.clear();
            if highlight {
                if let Some(m) = &self.matcher {
                    m.append_ranges(&full, &mut self.ranges);
                }
            }

            // 0) tag highlight backgrounds (behind the ad-hoc search highlight)
            for tag in &self.tags {
                if !tag.highlight_on {
                    continue;
                }
                let Some(matcher) = &tag.matcher else { continue };
                if matcher.is_empty() || !matcher.is_valid() {
                    continue;
                }

                self.tag_ranges.clear();
                matcher.append_ranges(&full, &mut self.tag_ranges);
                if self.tag_ranges.is_empty() {
                    continue;
                }

                let [r8, g8, b8] = tag.color();
                let brush = Color32::from_rgba_unmultiplied(r8, g8, b8, 70);
                for pair in self.tag_ranges.chunks_exact(2) {
                    let c0 = (pair[0] - self.left_col as i32).max(0);
                    let c1 = (pair[0] + pair[1] - self.left_col as i32).min(cols as i32);
                    if c1 <= c0 {
                        continue;
                    }
                    painter.rect_filled(
                        Rect::from_min_size(
                            pos2(text_x + c0 as f32 * self.char_w, y),
                            vec2((c1 - c0) as f32 * self.char_w, self.row_h),
                        ),
                        0.0,
                        brush,
                    );
                }
            }

            // 1) match backgrounds
            for pair in self.ranges.chunks_exact(2) {
                let c0 = (pair[0] - self.left_col as i32).max(0);
                let c1 = (pair[0] + pair[1] - self.left_col as i32).min(cols as i32);
                if c1 <= c0 {
                    continue;
                }
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(text_x + c0 as f32 * self.char_w, y),
                        vec2((c1 - c0) as f32 * self.char_w, self.row_h),
                    ),
                    0.0,
                    if is_current {
                        theme::CUR_BG
                    } else {
                        theme::MATCH_BG
                    },
                );
            }

            // 2) the line itself
            let visible = char_slice(&full, self.left_col, self.left_col + cols);
            painter.text(
                pos2(text_x, ty),
                Align2::LEFT_TOP,
                visible,
                self.font.clone(),
                theme::TEXT,
            );

            // 3) matched runs repainted in the highlight ink (exact x — the font is monospace)
            let full_chars = full.chars().count() as i32;
            for pair in self.ranges.chunks_exact(2) {
                let a = pair[0].max(self.left_col as i32);
                let b = (pair[0] + pair[1])
                    .min(self.left_col as i32 + cols as i32)
                    .min(full_chars);
                if b <= a {
                    continue;
                }
                painter.text(
                    pos2(
                        text_x + (a - self.left_col as i32) as f32 * self.char_w,
                        ty,
                    ),
                    Align2::LEFT_TOP,
                    char_slice(&full, a as usize, b as usize),
                    self.font.clone(),
                    if is_current {
                        theme::CUR_FG
                    } else {
                        theme::MATCH_FG
                    },
                );
            }
        }
    }

    /// The zoomed-out view: one row per template, count in the gutter, rare patterns starred and
    /// drawn in the accent colour.
    fn paint_templates(&self, painter: &eframe::egui::Painter, content: Rect) {
        if self.templates.is_empty() {
            painter.text(
                content.center(),
                Align2::CENTER_CENTER,
                "Clustering…",
                self.font.clone(),
                theme::TEXT_FAINT,
            );
            return;
        }

        painter.rect_filled(
            Rect::from_min_size(content.min, vec2(self.gutter_w, self.view_h)),
            0.0,
            theme::GUTTER,
        );
        theme::hairline(
            painter,
            theme::BORDER,
            pos2(content.left() + self.gutter_w, content.top()),
            pos2(content.left() + self.gutter_w, content.bottom()),
        );

        let rows = self.visible_rows();
        let pad = (self.row_h - self.text_h) / 2.0;
        let text_x = content.left() + self.text_x_offset();
        let cols = (((self.view_w - self.text_x_offset()) / self.char_w) as usize).max(1);

        for r in 0..rows {
            let i = self.top + r;
            let Some(template) = self.templates.get(i) else {
                break;
            };

            let y = content.top() + r as f32 * self.row_h;
            let ty = y + pad;
            let rare = template.count() <= self.rare_threshold;
            let is_selected = i as i64 == self.selected;

            if is_selected {
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(content.left() + self.gutter_w + 1.0, y),
                        vec2(self.view_w - self.gutter_w - 1.0, self.row_h),
                    ),
                    0.0,
                    theme::SEL_BG,
                );
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(content.left() + self.gutter_w + 1.0, y),
                        vec2(2.0, self.row_h),
                    ),
                    0.0,
                    theme::ACCENT,
                );
            }

            painter.text(
                pos2(content.left() + self.gutter_w - 10.0, ty),
                Align2::RIGHT_TOP,
                thousands(template.count() as u64),
                self.font.clone(),
                if rare { theme::ACCENT } else { theme::TEXT_DIM },
            );

            let rendered = template.render();
            let text = if rare {
                format!("★  {rendered}")
            } else {
                rendered
            };
            painter.text(
                pos2(text_x, ty),
                Align2::LEFT_TOP,
                char_slice(&text, 0, cols),
                self.font.clone(),
                if rare { theme::ACCENT } else { theme::TEXT },
            );
        }
    }

    fn paint_empty_state(&self, painter: &eframe::egui::Painter, content: Rect) {
        painter.text(
            content.center(),
            Align2::CENTER_CENTER,
            "Drop a log file here    ·    Ctrl+O to open",
            self.font.clone(),
            theme::TEXT_FAINT,
        );
    }
}

/// Characters `start..end` of `s`, clamped. Column arithmetic throughout the view is in characters,
/// so this is the only correct way to slice a line that may hold multi-byte UTF-8.
fn char_slice(s: &str, start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }
    s.chars().skip(start).take(end - start).collect()
}
