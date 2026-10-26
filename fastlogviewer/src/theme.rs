//! Colours, fonts and the two drawing primitives the custom controls share.
//!
//! The original had a `Theme.Px(v)` helper that multiplied every layout constant by the monitor
//! scale, because WinForms laid out in raw device pixels. egui lays out in *points* and applies
//! `pixels_per_point` itself, so every constant below is used verbatim and the scaling is gone —
//! not lost, just moved into the framework.

use eframe::egui::{Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, Rounding, Stroke};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const GUTTER: Color32 = Color32::from_rgb(0xF3, 0xF4, 0xF6);
pub const CHROME: Color32 = Color32::from_rgb(0xF6, 0xF7, 0xF9);
pub const CHROME_HI: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const BORDER: Color32 = Color32::from_rgb(0xDD, 0xE1, 0xE6);
pub const TEXT: Color32 = Color32::from_rgb(0x24, 0x29, 0x2E);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x5B, 0x63, 0x6C);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0xA1, 0xA8, 0xB0);
pub const ACCENT: Color32 = Color32::from_rgb(0x2A, 0x66, 0xE0);
pub const ACCENT_BG: Color32 = Color32::from_rgb(0xDE, 0xE9, 0xFC);
pub const SEL_BG: Color32 = Color32::from_rgb(0xE6, 0xEE, 0xFB);
pub const MATCH_BG: Color32 = Color32::from_rgb(0xFD, 0xEA, 0xA8);
pub const MATCH_FG: Color32 = Color32::from_rgb(0x6B, 0x51, 0x00);
pub const CUR_BG: Color32 = Color32::from_rgb(0xF5, 0xA8, 0x23);
pub const CUR_FG: Color32 = Color32::from_rgb(0x1A, 0x14, 0x00);
pub const ERROR: Color32 = Color32::from_rgb(0xC0, 0x30, 0x3D);
pub const THUMB: Color32 = Color32::from_rgb(0xC9, 0xCE, 0xD4);
pub const THUMB_HOT: Color32 = Color32::from_rgb(0xAE, 0xB5, 0xBD);
pub const TICK: Color32 = Color32::from_rgb(0xE0, 0x92, 0x00);

/// Thickness of both thin scrollbars.
pub const SCROLLBAR: f32 = 14.0;

/// WinForms font sizes are in points; egui's are in logical pixels. 96 dpi / 72 pt is the exact
/// conversion, so `mono(10.0)` is the same visual size the C# `Theme.Mono(10f)` produced.
fn pt(size: f32) -> f32 {
    size * 96.0 / 72.0
}

pub fn mono(size: f32) -> FontId {
    FontId::new(pt(size), FontFamily::Monospace)
}

pub fn ui_font(size: f32) -> FontId {
    FontId::new(pt(size), FontFamily::Proportional)
}

/// Installs JetBrains Mono and Inter ahead of egui's built-in faces, which stay as the fallback
/// for glyphs these two don't cover. Both are embedded so the look doesn't depend on what the
/// machine has installed.
pub fn install_fonts(ctx: &eframe::egui::Context) {
    let mut fonts = eframe::egui::FontDefinitions::default();
    add_font(
        &mut fonts,
        FontFamily::Monospace,
        "JetBrainsMono-Regular",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    add_font(
        &mut fonts,
        FontFamily::Proportional,
        "Inter-Regular",
        include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    );
    ctx.set_fonts(fonts);
}

fn add_font(
    fonts: &mut eframe::egui::FontDefinitions,
    family: FontFamily,
    name: &str,
    bytes: &'static [u8],
) {
    fonts.font_data.insert(
        name.to_string(),
        Arc::new(eframe::egui::FontData::from_static(bytes)),
    );
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, name.to_string());
}

pub fn fill_round(painter: &Painter, color: Color32, rect: Rect, radius: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let radius = radius.min(rect.width().min(rect.height()) / 2.0);
    painter.rect_filled(rect, Rounding::same(radius), color);
}

pub fn draw_round(painter: &Painter, color: Color32, rect: Rect, radius: f32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let radius = radius.min(rect.width().min(rect.height()) / 2.0);
    painter.rect_stroke(rect, Rounding::same(radius), Stroke::new(1.0, color));
}

pub fn hairline(painter: &Painter, color: Color32, from: Pos2, to: Pos2) {
    painter.line_segment([from, to], Stroke::new(1.0, color));
}

/// Centres one line of text in `rect` on the given axis, the common case the C# reached with
/// `TextFormatFlags.VerticalCenter`.
pub fn text_in(
    painter: &Painter,
    rect: Rect,
    align: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) {
    let pos = match align {
        Align2::LEFT_CENTER => Pos2::new(rect.left(), rect.center().y),
        Align2::RIGHT_CENTER => Pos2::new(rect.right(), rect.center().y),
        _ => rect.center(),
    };
    painter.text(pos, align, text, font, color);
}
