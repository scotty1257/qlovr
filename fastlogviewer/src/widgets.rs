//! The hand-drawn controls from the original: a flat scrollbar with match ticks, a pill toggle, a
//! focus-aware field frame, and the status strip. Each was a `Control` subclass that owned its own
//! paint and hit-testing; here each is a function over a rect plus the small amount of state that
//! genuinely has to survive between frames (the scrollbar's drag).

use eframe::egui::{Align2, Color32, FontId, Id, Rect, Response, Sense, Ui, Vec2};
use fastlogviewer_core::{FilterIndex, MatchIndex};

use crate::theme;

/// Flat themed scrollbar. Same `value`/`maximum`/`large_change` semantics as WinForms, including
/// the "page" jump when you click the track outside the thumb.
pub struct ThinScrollBar {
    vertical: bool,
    value: i32,
    maximum: i32,
    large: i32,
    small: i32,
    drag_offset: f32,
}

impl ThinScrollBar {
    pub fn new(vertical: bool) -> Self {
        ThinScrollBar {
            vertical,
            value: 0,
            maximum: 0,
            large: 1,
            small: 1,
            drag_offset: 0.0,
        }
    }

    pub fn value(&self) -> i32 {
        self.value
    }

    pub fn maximum(&self) -> i32 {
        self.maximum
    }

    pub fn max_value(&self) -> i32 {
        (self.maximum - self.large + 1).max(0)
    }

    pub fn set_maximum(&mut self, v: i32) {
        self.maximum = v.max(0);
        self.clamp();
    }

    pub fn set_large_change(&mut self, v: i32) {
        self.large = v.max(1);
        self.clamp();
    }

    pub fn set_small_change(&mut self, v: i32) {
        self.small = v.max(1);
    }

    /// Returns true when the value actually moved, which is what the C# `ValueChanged` event did.
    pub fn set_value(&mut self, v: i32) -> bool {
        let clamped = v.clamp(0, self.max_value());
        if clamped == self.value {
            return false;
        }
        self.value = clamped;
        true
    }

    fn clamp(&mut self) {
        if self.value > self.max_value() {
            self.set_value(self.max_value());
        }
    }

    fn track(&self, rect: Rect) -> f32 {
        if self.vertical {
            rect.height()
        } else {
            rect.width()
        }
    }

    fn thumb_size(&self, rect: Rect) -> f32 {
        let track = self.track(rect);
        let total = self.maximum + 1;
        if total <= 0 {
            return track;
        }
        let s = track * self.large as f32 / total as f32;
        s.max(28.0).min(track)
    }

    fn thumb_pos(&self, rect: Rect) -> f32 {
        let span = self.track(rect) - self.thumb_size(rect);
        if self.max_value() <= 0 || span <= 0.0 {
            return 0.0;
        }
        span * self.value as f32 / self.max_value() as f32
    }

    /// Paints into `rect` and handles its own dragging. `marks`/`mark_total` draw the match ticks on
    /// a vertical track; they are meaningless when filtering, where the caller passes `None`.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Id,
        marks: Option<&MatchIndex>,
        mark_total: i32,
    ) -> bool {
        let response: Response = ui.interact(rect, id, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::BG);

        if self.vertical {
            if let Some(marks) = marks {
                let n = marks.count();
                if n > 0 && mark_total > 0 {
                    let step = (n / 1200).max(1);
                    let h = (rect.height() - 2.0).max(1.0);
                    let (tw, th) = (4.0, 2.0);
                    let mut i = 0;
                    while i < n {
                        let y = rect.top() + marks.get(i) as f32 * h / mark_total as f32;
                        painter.rect_filled(
                            Rect::from_min_size(
                                eframe::egui::pos2(rect.right() - tw - 1.0, y),
                                Vec2::new(tw, th),
                            ),
                            0.0,
                            theme::TICK,
                        );
                        i += step;
                    }
                }
            }
        }

        let mut changed = false;
        if self.max_value() > 0 {
            let thumb_pos = self.thumb_pos(rect);
            let thumb_size = self.thumb_size(rect);

            if let Some(pointer) = response.interact_pointer_pos() {
                let along = if self.vertical {
                    pointer.y - rect.top()
                } else {
                    pointer.x - rect.left()
                };

                if response.drag_started() || response.clicked() {
                    if along >= thumb_pos && along < thumb_pos + thumb_size {
                        self.drag_offset = along - thumb_pos;
                    } else {
                        // Click above/below the thumb pages by one screenful.
                        let page = if along < thumb_pos {
                            -self.large
                        } else {
                            self.large
                        };
                        changed |= self.set_value(self.value + page);
                    }
                } else if response.dragged() {
                    let span = self.track(rect) - thumb_size;
                    if span > 0.0 {
                        let pos = (along - self.drag_offset).clamp(0.0, span);
                        changed |= self
                            .set_value((pos * self.max_value() as f32 / span).round() as i32);
                    }
                }
            }

            let hot = response.hovered() || response.dragged();
            let inset = 4.0;
            let thumb_pos = self.thumb_pos(rect);
            let thumb_size = self.thumb_size(rect);
            let thumb = if self.vertical {
                Rect::from_min_size(
                    eframe::egui::pos2(rect.left() + inset, rect.top() + thumb_pos),
                    Vec2::new((rect.width() - inset * 2.0).max(1.0), thumb_size),
                )
            } else {
                Rect::from_min_size(
                    eframe::egui::pos2(rect.left() + thumb_pos, rect.top() + inset),
                    Vec2::new(thumb_size, (rect.height() - inset * 2.0).max(1.0)),
                )
            };
            theme::fill_round(
                &painter,
                if hot { theme::THUMB_HOT } else { theme::THUMB },
                thumb,
                3.0,
            );
        }

        changed
    }
}

/// Pill toggle: accent fill and border when on, a lighter fill on hover. Returns true when clicked,
/// having already flipped `checked`.
pub fn toggle_button(
    ui: &mut Ui,
    rect: Rect,
    text: &str,
    font: FontId,
    checked: &mut bool,
    tooltip: &str,
) -> bool {
    let response = ui.interact(rect, ui.id().with(text).with("toggle"), Sense::click());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 0.0, theme::CHROME);
    let inner = Rect::from_min_max(rect.min, rect.max - Vec2::splat(1.0));
    let radius = 5.0;

    let fill = if *checked {
        theme::ACCENT_BG
    } else if response.hovered() {
        theme::CHROME_HI
    } else {
        theme::CHROME
    };
    theme::fill_round(&painter, fill, inner, radius);
    theme::draw_round(
        &painter,
        if *checked { theme::ACCENT } else { theme::BORDER },
        inner,
        radius,
    );

    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font,
        if *checked { theme::TEXT } else { theme::TEXT_DIM },
    );

    let response = if tooltip.is_empty() {
        response
    } else {
        response.on_hover_text(tooltip)
    };

    if response.clicked() {
        *checked = !*checked;
        return true;
    }
    false
}

/// Rounded, focus-aware container for the search text box. Returns the inner rect the caller should
/// place the editor in.
pub fn field_frame(ui: &mut Ui, rect: Rect, focused: bool) -> Rect {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CHROME);
    let inner = Rect::from_min_max(rect.min, rect.max - Vec2::splat(1.0));
    theme::fill_round(&painter, theme::CHROME_HI, inner, 6.0);
    theme::draw_round(
        &painter,
        if focused { theme::ACCENT } else { theme::BORDER },
        inner,
        6.0,
    );

    Rect::from_min_max(
        eframe::egui::pos2(rect.left() + 10.0, rect.top()),
        eframe::egui::pos2(rect.right() - 10.0, rect.bottom()),
    )
}

/// Status bar: background, hairline top border, left and right text.
pub fn status_strip(
    ui: &mut Ui,
    rect: Rect,
    left: &str,
    right: &str,
    left_color: Color32,
    font: FontId,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::CHROME);
    theme::hairline(&painter, theme::BORDER, rect.left_top(), rect.right_top());

    let text_rect = Rect::from_min_max(
        eframe::egui::pos2(rect.left() + 12.0, rect.top() + 1.0),
        eframe::egui::pos2(rect.right() - 12.0, rect.bottom()),
    );
    theme::text_in(
        &painter,
        text_rect,
        Align2::LEFT_CENTER,
        left,
        font.clone(),
        left_color,
    );
    theme::text_in(
        &painter,
        text_rect,
        Align2::RIGHT_CENTER,
        right,
        font,
        theme::TEXT_FAINT,
    );
}

/// `1234567` -> `"1,234,567"`, the .NET `"N0"` format the status line and gutter rely on.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// .NET `"N1"`: grouped integer part, one decimal place.
pub fn thousands_1dp(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    let whole = rounded.trunc().abs() as u64;
    let frac = ((rounded.abs() - whole as f64) * 10.0).round() as u64;
    let sign = if rounded < 0.0 { "-" } else { "" };
    format!("{sign}{}.{frac}", thousands(whole))
}
