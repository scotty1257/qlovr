//! Tag chips and the tag editor dialog.

use eframe::egui::{pos2, vec2, Align2, Color32, Context, FontId, Id, Rect, Sense, Ui, Vec2};
use fastlogviewer_core::{Matcher, TagDef, TAG_PALETTE};
use uuid::Uuid;

use crate::theme;

pub const CHIP_HEIGHT: f32 = 26.0;
pub const CHIP_MARGIN: f32 = 4.0;

#[derive(PartialEq, Eq, Debug)]
pub enum ChipAction {
    None,
    ToggleHighlight,
    ToggleFilter,
    Remove,
    Edit,
}

fn text_width(ctx: &Context, text: &str, font: &FontId) -> f32 {
    ctx.fonts(|f| {
        f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    })
}

/// label + H + F + x + padding, the same measurement the C# chip did in `MeasureWidth`.
pub fn chip_width(ctx: &Context, tag: &TagDef, font: &FontId) -> f32 {
    text_width(ctx, &tag.label, font) + 70.0
}

pub fn tag_chip(ui: &mut Ui, tag: &TagDef, rect: Rect, font: FontId) -> ChipAction {
    let response = ui.interact(rect, Id::new(("chip", tag.id)), Sense::click());
    let pointer = ui.ctx().pointer_latest_pos();

    let [r, g, b] = tag.color();
    let base = Color32::from_rgb(r, g, b);
    let bg = Color32::from_rgba_unmultiplied(r, g, b, 40);

    let inner = Rect::from_min_max(rect.min, rect.max - Vec2::splat(1.0));
    let painter = ui.painter_at(rect);
    theme::fill_round(&painter, bg, inner, 6.0);
    theme::draw_round(&painter, base, inner, 6.0);

    let label_w = text_width(ui.ctx(), &tag.label, &font);
    let mut x = rect.left() + 8.0;

    theme::text_in(
        &painter,
        Rect::from_min_size(pos2(x, rect.top()), vec2(label_w, rect.height())),
        Align2::LEFT_CENTER,
        &tag.label,
        font.clone(),
        theme::TEXT,
    );
    x += label_w + 8.0;

    let h_rect = Rect::from_min_size(pos2(x, rect.top()), vec2(16.0, rect.height()));
    x += 16.0;
    let f_rect = Rect::from_min_size(pos2(x, rect.top()), vec2(16.0, rect.height()));
    x += 16.0 + 4.0;
    let x_rect = Rect::from_min_size(pos2(x, rect.top()), vec2(18.0, rect.height()));

    let hovers = |r: Rect| pointer.is_some_and(|p| r.contains(p));

    draw_glyph(
        &painter,
        h_rect,
        "H",
        tag.highlight_on,
        base,
        hovers(h_rect),
        font.clone(),
    );
    draw_glyph(
        &painter,
        f_rect,
        "F",
        tag.filter_on,
        base,
        hovers(f_rect),
        font.clone(),
    );
    painter.text(
        x_rect.center(),
        Align2::CENTER_CENTER,
        "×",
        font,
        if hovers(x_rect) {
            theme::ERROR
        } else {
            theme::TEXT_FAINT
        },
    );

    if !response.clicked() {
        return ChipAction::None;
    }
    match response.interact_pointer_pos() {
        Some(p) if x_rect.contains(p) => ChipAction::Remove,
        Some(p) if h_rect.contains(p) => ChipAction::ToggleHighlight,
        Some(p) if f_rect.contains(p) => ChipAction::ToggleFilter,
        _ => ChipAction::Edit,
    }
}

fn draw_glyph(
    painter: &eframe::egui::Painter,
    rect: Rect,
    text: &str,
    on: bool,
    on_color: Color32,
    hover: bool,
    font: FontId,
) {
    let color = if on {
        on_color
    } else if hover {
        theme::TEXT_DIM
    } else {
        theme::TEXT_FAINT
    };
    painter.text(rect.center(), Align2::CENTER_CENTER, text, font, color);
}

pub fn add_tag_chip(ui: &mut Ui, rect: Rect, font: FontId) -> bool {
    let response = ui.interact(rect, Id::new("add_tag_chip"), Sense::click());
    let painter = ui.painter_at(rect);
    let inner = Rect::from_min_max(rect.min, rect.max - Vec2::splat(1.0));
    theme::fill_round(&painter, theme::CHROME, inner, 6.0);
    theme::draw_round(&painter, theme::BORDER, inner, 6.0);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "+",
        font,
        theme::TEXT_DIM,
    );
    response.clicked()
}

// --------------------------------------------------------------- TagEditPopup

pub enum PopupOutcome {
    Open,
    Accepted,
    Cancelled,
}

/// The new/edit tag dialog. The original placed every control at a hand-computed offset inside a
/// 300x224 `FixedDialog`; here egui lays the same controls out vertically inside a modal window.
pub struct TagEditPopup {
    /// `None` for a new tag, otherwise the id of the tag being edited.
    pub editing: Option<Uuid>,
    pub label: String,
    pub pattern: String,
    /// The "Aa" toggle. It carries the C#'s meaning: on means *ignore* case.
    pub ignore_case: bool,
    pub highlight: bool,
    pub filter: bool,
    pub color_index: usize,
}

impl TagEditPopup {
    pub fn new_tag(default_color_index: usize) -> Self {
        TagEditPopup {
            editing: None,
            label: String::new(),
            pattern: String::new(),
            ignore_case: false,
            highlight: true,
            filter: false,
            color_index: default_color_index,
        }
    }

    pub fn edit(tag: &TagDef) -> Self {
        TagEditPopup {
            editing: Some(tag.id),
            label: tag.label.clone(),
            pattern: tag.pattern.clone(),
            ignore_case: tag.ignore_case,
            highlight: tag.highlight_on,
            filter: tag.filter_on,
            color_index: tag.color_index,
        }
    }

    /// Blank label falls back to the pattern, matching the C# `Commit`.
    pub fn result_label(&self) -> String {
        if self.label.trim().is_empty() {
            self.pattern.clone()
        } else {
            self.label.trim().to_string()
        }
    }

    pub fn show(&mut self, ctx: &Context) -> PopupOutcome {
        let title = if self.editing.is_none() {
            "New tag"
        } else {
            "Edit tag"
        };

        let matcher = Matcher::new(&self.pattern, true, self.ignore_case);
        let error = matcher.error.clone().unwrap_or_default();
        let can_accept = !matcher.is_empty() && matcher.is_valid();

        let mut outcome = PopupOutcome::Open;
        let mut open = true;

        eframe::egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.label("Label");
                ui.add(
                    eframe::egui::TextEdit::singleline(&mut self.label)
                        .desired_width(f32::INFINITY),
                );

                ui.add_space(8.0);
                ui.label("Pattern (regex)");
                ui.horizontal(|ui| {
                    ui.add(
                        eframe::egui::TextEdit::singleline(&mut self.pattern)
                            .font(theme::mono(9.0))
                            .desired_width(ui.available_width() - 48.0),
                    );
                    let rect = Rect::from_min_size(
                        ui.cursor().min,
                        vec2(40.0, 24.0),
                    );
                    ui.allocate_rect(rect, Sense::hover());
                    crate::widgets::toggle_button(
                        ui,
                        rect,
                        "Aa",
                        theme::mono(9.0),
                        &mut self.ignore_case,
                        "Match case",
                    );
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.highlight, "Highlight");
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.filter, "Filter");
                });

                ui.add_space(8.0);
                self.swatches(ui);

                ui.add_space(6.0);
                ui.colored_label(theme::ERROR, &error);

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() - 150.0);
                    if ui.button("Cancel").clicked() {
                        outcome = PopupOutcome::Cancelled;
                    }
                    if ui
                        .add_enabled(can_accept, eframe::egui::Button::new("OK"))
                        .clicked()
                    {
                        outcome = PopupOutcome::Accepted;
                    }
                });
            });

        if !open {
            return PopupOutcome::Cancelled;
        }
        if ctx.input(|i| i.key_pressed(eframe::egui::Key::Escape)) {
            return PopupOutcome::Cancelled;
        }
        if can_accept && ctx.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
            return PopupOutcome::Accepted;
        }
        outcome
    }

    fn swatches(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for (i, [r, g, b]) in TAG_PALETTE.iter().enumerate() {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, Color32::from_rgb(*r, *g, *b));
                if i == self.color_index {
                    theme::draw_round(&painter, theme::TEXT, rect, 0.0);
                }
                if response.clicked() {
                    self.color_index = i;
                }
                ui.add_space(2.0);
            }
        });
    }
}
