//! Colours, fonts and the widget styles the window is built from.
//!
//! Everything visual lives here so the layout code in `app.rs` and `tag_ui.rs` only says *which*
//! style a widget gets, never what it looks like. The palette is a dark graphite with one blue
//! accent; log content keeps the warm amber match highlight the original used, because it reads
//! well against both the text and the selection.

use iced::border::Radius;
use iced::widget::{button, checkbox, container, svg, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector};

const fn hex(rgb: u32) -> Color {
    Color {
        r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
        g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
        b: (rgb & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

pub const fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

// Surfaces, darkest to lightest.
pub const BG: Color = hex(0x0E1116);
pub const GUTTER: Color = hex(0x11151B);
pub const SURFACE: Color = hex(0x151920);
pub const RAISED: Color = hex(0x1B2029);
pub const HOVER: Color = hex(0x232935);
pub const BORDER: Color = hex(0x252B36);
pub const BORDER_STRONG: Color = hex(0x343C4A);

// Ink.
pub const TEXT: Color = hex(0xD5DAE1);
pub const TEXT_DIM: Color = hex(0x8A93A3);
pub const TEXT_FAINT: Color = hex(0x565F70);

pub const ACCENT: Color = hex(0x5B9BFF);
pub const ON_ACCENT: Color = hex(0x0B1220);
pub const SEL_BG: Color = hex(0x1A2436);
pub const HOVER_ROW: Color = hex(0x141922);

pub const MATCH_BG: Color = Color {
    r: 1.0,
    g: 0.77,
    b: 0.18,
    a: 0.20,
};
pub const MATCH_FG: Color = hex(0xFFD978);
pub const CUR_BG: Color = hex(0xF5A623);
pub const CUR_FG: Color = hex(0x16110A);
pub const TICK: Color = hex(0xE8A33A);

// Log levels.
pub const ERROR: Color = hex(0xFF6B6B);
pub const WARN: Color = hex(0xF2C14E);
pub const INFO: Color = hex(0x58C4A8);
pub const DEBUG: Color = hex(0x8A93A3);

pub const MONO: Font = Font::with_name("JetBrains Mono");
pub const UI: Font = Font::with_name("Inter");

pub const MONO_SIZE: f32 = 13.0;
pub const UI_SIZE: f32 = 13.0;
pub const SMALL_SIZE: f32 = 12.0;

pub const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
pub const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");

/// The base iced theme. Most widgets get an explicit style below; this only matters for the few
/// defaults that leak through (text colour, selection, the window background).
pub fn app_theme() -> Theme {
    Theme::custom(
        "FastLogViewer",
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: INFO,
            warning: WARN,
            danger: ERROR,
        },
    )
}

fn border(color: Color, width: f32, radius: f32) -> Border {
    Border {
        color,
        width,
        radius: Radius::from(radius),
    }
}

// ----------------------------------------------------------------- containers

pub fn surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SURFACE.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn log_surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BG.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn divider(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BORDER.into()),
        ..Default::default()
    }
}

/// The strip that names the current zoom level (patterns list, or one pattern's lines).
pub fn breadcrumb(_: &Theme) -> container::Style {
    container::Style {
        background: Some(alpha(ACCENT, 0.07).into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn pill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(color, 0.16).into()),
        text_color: Some(color),
        border: border(alpha(color, 0.35), 1.0, 999.0),
        ..Default::default()
    }
}

pub fn segmented(_: &Theme) -> container::Style {
    container::Style {
        background: Some(RAISED.into()),
        border: border(BORDER, 1.0, 8.0),
        ..Default::default()
    }
}

pub fn kbd(_: &Theme) -> container::Style {
    container::Style {
        background: Some(RAISED.into()),
        text_color: Some(TEXT_DIM),
        border: border(BORDER_STRONG, 1.0, 4.0),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(HOVER.into()),
        text_color: Some(TEXT),
        border: border(BORDER_STRONG, 1.0, 6.0),
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.4),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    }
}

pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(alpha(Color::BLACK, 0.55).into()),
        ..Default::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(SURFACE.into()),
        text_color: Some(TEXT),
        border: border(BORDER_STRONG, 1.0, 12.0),
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.5),
            offset: Vector::new(0.0, 12.0),
            blur_radius: 40.0,
        },
        ..Default::default()
    }
}

/// A tag chip: tinted with the tag's own colour so the chip row doubles as a legend for the
/// highlights in the log.
pub fn chip(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(alpha(color, 0.12).into()),
        text_color: Some(TEXT),
        border: border(alpha(color, 0.45), 1.0, 8.0),
        ..Default::default()
    }
}

pub fn dot(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(color.into()),
        border: border(color, 0.0, 999.0),
        ..Default::default()
    }
}

// -------------------------------------------------------------------- buttons

fn button_style(background: Color, text: Color, edge: Color) -> button::Style {
    button::Style {
        background: Some(Background::Color(background)),
        text_color: text,
        border: border(edge, 1.0, 6.0),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Toolbar toggles (`.*`, `Aa`, `Filter`, `Patterns`): accent-tinted when on.
pub fn toggle(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        match (on, hovered) {
            (true, _) => button_style(alpha(ACCENT, 0.18), ACCENT, alpha(ACCENT, 0.0)),
            (false, true) => button_style(HOVER, TEXT, alpha(HOVER, 0.0)),
            (false, false) => button_style(alpha(RAISED, 0.0), TEXT_DIM, alpha(RAISED, 0.0)),
        }
    }
}

/// Borderless icon or text button that only shows a surface on hover.
pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(HOVER, TEXT, alpha(HOVER, 0.0))
        }
        _ => button_style(alpha(SURFACE, 0.0), TEXT_DIM, alpha(SURFACE, 0.0)),
    }
}

/// Outlined secondary action ("+ New tag", "Cancel").
pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(HOVER, TEXT, BORDER_STRONG)
        }
        _ => button_style(RAISED, TEXT_DIM, BORDER),
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(hex(0x78AEFF), ON_ACCENT, hex(0x78AEFF))
        }
        button::Status::Disabled => {
            button_style(alpha(ACCENT, 0.25), alpha(ON_ACCENT, 0.6), alpha(ACCENT, 0.0))
        }
        button::Status::Active => button_style(ACCENT, ON_ACCENT, ACCENT),
    }
}

/// The label part of a tag chip: no surface of its own, the chip provides it.
pub fn chip_label(_: &Theme, status: button::Status) -> button::Style {
    let text = match status {
        button::Status::Hovered | button::Status::Pressed => Color::WHITE,
        _ => TEXT,
    };
    button_style(alpha(SURFACE, 0.0), text, alpha(SURFACE, 0.0))
}

/// The H / F switches inside a chip: filled with the tag colour when on.
/// `implied` is a level tag's F switch that is off but included by a less severe level's filter:
/// outlined rather than filled.
pub fn chip_switch(
    on: bool,
    implied: bool,
    color: Color,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let mut style = match (on, hovered) {
            (true, _) => button_style(color, ON_ACCENT, color),
            (false, _) if implied => button_style(alpha(color, 0.18), TEXT, color),
            (false, true) => button_style(alpha(color, 0.22), TEXT, alpha(color, 0.0)),
            (false, false) => button_style(alpha(color, 0.0), TEXT_DIM, alpha(color, 0.0)),
        };
        style.border.radius = Radius::from(4.0);
        style
    }
}

pub fn chip_remove(_: &Theme, status: button::Status) -> button::Style {
    let mut style = match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(alpha(ERROR, 0.18), ERROR, alpha(ERROR, 0.0))
        }
        _ => button_style(alpha(ERROR, 0.0), TEXT_FAINT, alpha(ERROR, 0.0)),
    };
    style.border.radius = Radius::from(4.0);
    style
}

pub fn swatch(color: Color, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let ring = if selected {
            Color::WHITE
        } else if hovered {
            alpha(Color::WHITE, 0.5)
        } else {
            alpha(color, 0.0)
        };
        button::Style {
            background: Some(color.into()),
            text_color: ON_ACCENT,
            border: border(ring, 2.0, 999.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

// --------------------------------------------------------------------- inputs

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let edge = match status {
        text_input::Status::Focused { .. } => ACCENT,
        text_input::Status::Hovered => BORDER_STRONG,
        _ => BORDER,
    };
    text_input::Style {
        background: RAISED.into(),
        border: border(edge, 1.0, 8.0),
        icon: TEXT_DIM,
        placeholder: TEXT_FAINT,
        value: TEXT,
        selection: alpha(ACCENT, 0.35),
    }
}

pub fn check(_: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, hovered) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true),
        checkbox::Status::Disabled { is_checked } => (is_checked, false),
    };
    checkbox::Style {
        background: if checked { ACCENT } else { RAISED }.into(),
        icon_color: ON_ACCENT,
        border: border(
            if checked {
                ACCENT
            } else if hovered {
                BORDER_STRONG
            } else {
                BORDER
            },
            1.0,
            4.0,
        ),
        text_color: Some(TEXT),
    }
}

pub fn icon(color: Color) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |_, _| svg::Style { color: Some(color) }
}

// ---------------------------------------------------------------------- icons

/// Stroke icons (Lucide-style, 24x24 grid). `svg::Style::color` tints them, so the stroke colour
/// written here does not matter.
pub mod icons {
    pub const SEARCH: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>"#;
    pub const FOLDER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>"#;
    pub const LAYERS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m12 2 10 5-10 5L2 7l10-5Z"/><path d="m2 17 10 5 10-5"/><path d="m2 12 10 5 10-5"/></svg>"#;
    pub const FILE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2Z"/><path d="M14 2v6h6"/><path d="M8 13h8"/><path d="M8 17h8"/><path d="M8 9h2"/></svg>"#;
    pub const PLUS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>"#;
}
