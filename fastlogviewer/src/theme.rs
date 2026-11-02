//! Colours, fonts and the widget styles the window is built from.
//!
//! Everything visual lives here so the layout code in `app.rs` and `tag_ui.rs` only says *which*
//! style a widget gets, never what it looks like. There are two palettes, a dark graphite and a
//! light paper, each with one blue accent; log content keeps the warm amber match highlight the
//! original used, because it reads well against both the text and the selection.
//!
//! The active palette is process-wide (`palette()`), so the style functions and the canvas can
//! keep their plain `fn(&Theme)` shapes. The choice is saved next to `tags.json`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use iced::border::Radius;
use iced::font::Weight;
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

pub struct Palette {
    // Surfaces, from the log background up to the strongest edge.
    pub bg: Color,
    pub gutter: Color,
    pub surface: Color,
    pub raised: Color,
    pub hover: Color,
    pub border: Color,
    pub border_strong: Color,

    // Ink. `text_strong` is the hover ink for chip labels and the selected swatch ring.
    pub text: Color,
    pub text_dim: Color,
    pub text_faint: Color,
    pub text_strong: Color,

    pub accent: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub sel_bg: Color,
    pub hover_row: Color,

    pub match_bg: Color,
    pub match_fg: Color,
    pub cur_bg: Color,
    pub cur_fg: Color,
    pub tick: Color,

    // Log levels.
    pub error: Color,
    pub warn: Color,
    pub info: Color,
    pub debug: Color,

    // Gutter marks for lines that changed on disk since they were last cleared.
    pub diff_added: Color,
    pub diff_changed: Color,
    pub diff_deleted: Color,

    /// Drop shadows and the modal backdrop; a light surface needs far less.
    pub shadow: f32,
}

const DARK: Palette = Palette {
    bg: hex(0x0E1116),
    gutter: hex(0x11151B),
    surface: hex(0x151920),
    raised: hex(0x1B2029),
    hover: hex(0x232935),
    border: hex(0x252B36),
    border_strong: hex(0x343C4A),

    text: hex(0xD5DAE1),
    text_dim: hex(0x8A93A3),
    text_faint: hex(0x565F70),
    text_strong: Color::WHITE,

    accent: hex(0x5B9BFF),
    accent_hover: hex(0x78AEFF),
    on_accent: hex(0x0B1220),
    sel_bg: hex(0x1A2436),
    hover_row: hex(0x141922),

    match_bg: Color {
        r: 1.0,
        g: 0.77,
        b: 0.18,
        a: 0.20,
    },
    match_fg: hex(0xFFD978),
    cur_bg: hex(0xF5A623),
    cur_fg: hex(0x16110A),
    tick: hex(0xE8A33A),

    error: hex(0xFF6B6B),
    warn: hex(0xF2C14E),
    info: hex(0x58C4A8),
    debug: hex(0x8A93A3),

    diff_added: hex(0x3FB950),
    diff_changed: hex(0x388BFD),
    diff_deleted: hex(0xF85149),

    shadow: 1.0,
};

const LIGHT: Palette = Palette {
    bg: hex(0xFFFFFF),
    gutter: hex(0xF6F7F9),
    surface: hex(0xF1F3F6),
    raised: hex(0xFFFFFF),
    hover: hex(0xE4E8EE),
    border: hex(0xDCE0E6),
    border_strong: hex(0xC3CAD4),

    text: hex(0x1E232B),
    text_dim: hex(0x596273),
    text_faint: hex(0x8D95A3),
    text_strong: hex(0x0E1116),

    accent: hex(0x2563D9),
    accent_hover: hex(0x3B78EA),
    on_accent: hex(0xFFFFFF),
    sel_bg: hex(0xDDE8FB),
    hover_row: hex(0xF3F6FA),

    match_bg: Color {
        r: 1.0,
        g: 0.77,
        b: 0.18,
        a: 0.38,
    },
    match_fg: hex(0x5C3D00),
    cur_bg: hex(0xF5A623),
    cur_fg: hex(0x16110A),
    tick: hex(0xD08A1A),

    error: hex(0xD03535),
    warn: hex(0xA87400),
    info: hex(0x14866B),
    debug: hex(0x6B7484),

    diff_added: hex(0x1A7F37),
    diff_changed: hex(0x0969DA),
    diff_deleted: hex(0xCF222E),

    shadow: 0.35,
};

static LIGHT_MODE: AtomicBool = AtomicBool::new(false);

pub fn palette() -> &'static Palette {
    if is_light() {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn is_light() -> bool {
    LIGHT_MODE.load(Ordering::Relaxed)
}

fn mode_path() -> Option<PathBuf> {
    // %APPDATA%\FastLogViewer, beside tags.json.
    dirs::config_dir().map(|d| d.join("FastLogViewer").join("theme.txt"))
}

/// Picks up the saved choice; anything missing or unreadable means dark.
pub fn load_mode() {
    let light = mode_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|saved| saved.trim() == "light");
    LIGHT_MODE.store(light, Ordering::Relaxed);
}

/// Switches palettes and saves the choice. Best-effort: a failed save only forgets it.
pub fn set_light(light: bool) {
    LIGHT_MODE.store(light, Ordering::Relaxed);
    let Some(path) = mode_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, if light { "light" } else { "dark" });
}

pub const MONO: Font = Font::with_name("IoskeleyMonoNL Nerd Font Mono");
/// The log text is set a weight up from the toolbar's mono labels, for slightly firmer strokes.
pub const LOG_MONO: Font = Font {
    weight: Weight::Medium,
    ..MONO
};
pub const UI: Font = Font::with_name("IBM Plex Sans");
/// Small UI text is set a weight up: Regular at 13px draws thin, washed-out strokes.
pub const UI_SMALL: Font = Font {
    weight: Weight::Medium,
    ..UI
};

/// Sized so an Ioskeley Mono cell (0.6 em) is exactly 8px: every column then starts on a whole
/// pixel and glyph stems land on the pixel grid instead of smearing across two pixels.
pub const MONO_SIZE: f32 = 40.0 / 3.0;
pub const UI_SIZE: f32 = 14.0;
pub const SMALL_SIZE: f32 = 13.0;

/// The single-width ("Mono") Nerd Font build, so icon glyphs keep to the 8px grid too.
pub const IOSKELEY_MONO: &[u8] =
    include_bytes!("../assets/fonts/IoskeleyMonoNLNerdFontMono-Regular.ttf");
pub const IOSKELEY_MONO_MEDIUM: &[u8] =
    include_bytes!("../assets/fonts/IoskeleyMonoNLNerdFontMono-Medium.ttf");
pub const IBM_PLEX: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
pub const IBM_PLEX_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf");

/// The base iced theme. Most widgets get an explicit style below; this only matters for the few
/// defaults that leak through (text colour, selection, the window background).
pub fn app_theme() -> Theme {
    let p = palette();
    Theme::custom(
        if is_light() {
            "FastLogViewer Light"
        } else {
            "FastLogViewer"
        },
        iced::theme::Palette {
            background: p.bg,
            text: p.text,
            primary: p.accent,
            success: p.info,
            warning: p.warn,
            danger: p.error,
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
        background: Some(palette().surface.into()),
        text_color: Some(palette().text),
        ..Default::default()
    }
}

pub fn log_surface(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().bg.into()),
        text_color: Some(palette().text),
        ..Default::default()
    }
}

pub fn divider(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().border.into()),
        ..Default::default()
    }
}

/// The row of tabs above each pane: a step darker than the toolbar, so the active tab, drawn in
/// the toolbar's colour, reads as joined to it.
pub fn tab_strip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().gutter.into()),
        text_color: Some(palette().text_dim),
        ..Default::default()
    }
}

/// One tab in a strip. `active` is the pane's shown tab.
pub fn tab(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: active.then(|| palette().surface.into()),
        text_color: Some(if active {
            palette().text
        } else {
            palette().text_dim
        }),
        ..Default::default()
    }
}

/// The bar along the top of the active tab: accent in the focused pane, quieter in the others.
pub fn tab_marker(active: bool, focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: match (active, focused) {
            (true, true) => Some(palette().accent.into()),
            (true, false) => Some(palette().border_strong.into()),
            _ => None,
        },
        ..Default::default()
    }
}

/// The 1px gaps between panes, and the split line while it is hovered or dragged.
pub fn pane_grid(_: &Theme) -> iced::widget::pane_grid::Style {
    use iced::widget::pane_grid::{Highlight, Line, Style};
    Style {
        hovered_region: Highlight {
            background: alpha(palette().accent, 0.18).into(),
            border: border(palette().accent, 2.0, 0.0),
        },
        hovered_split: Line {
            color: palette().accent,
            width: 2.0,
        },
        picked_split: Line {
            color: palette().accent_hover,
            width: 2.0,
        },
    }
}

/// The strip that names the current zoom level (patterns list, or one pattern's lines).
pub fn breadcrumb(_: &Theme) -> container::Style {
    container::Style {
        background: Some(alpha(palette().accent, 0.07).into()),
        text_color: Some(palette().text),
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
        background: Some(palette().raised.into()),
        border: border(palette().border, 1.0, 8.0),
        ..Default::default()
    }
}

pub fn kbd(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().raised.into()),
        text_color: Some(palette().text_dim),
        border: border(palette().border_strong, 1.0, 4.0),
        ..Default::default()
    }
}

pub fn tooltip(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().hover.into()),
        text_color: Some(palette().text),
        border: border(palette().border_strong, 1.0, 6.0),
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.4 * palette().shadow),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    }
}

pub fn backdrop(_: &Theme) -> container::Style {
    container::Style {
        background: Some(alpha(Color::BLACK, 0.55 * palette().shadow).into()),
        ..Default::default()
    }
}

pub fn card(_: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().surface.into()),
        text_color: Some(palette().text),
        border: border(palette().border_strong, 1.0, 12.0),
        shadow: Shadow {
            color: alpha(Color::BLACK, 0.5 * palette().shadow),
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
        text_color: Some(palette().text),
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
            (true, _) => button_style(
                alpha(palette().accent, 0.18),
                palette().accent,
                alpha(palette().accent, 0.0),
            ),
            (false, true) => {
                button_style(palette().hover, palette().text, alpha(palette().hover, 0.0))
            }
            (false, false) => button_style(
                alpha(palette().raised, 0.0),
                palette().text_dim,
                alpha(palette().raised, 0.0),
            ),
        }
    }
}

/// Borderless icon or text button that only shows a surface on hover.
pub fn ghost(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(palette().hover, palette().text, alpha(palette().hover, 0.0))
        }
        _ => button_style(
            alpha(palette().surface, 0.0),
            palette().text_dim,
            alpha(palette().surface, 0.0),
        ),
    }
}

/// Outlined secondary action ("+ New tag", "Cancel").
pub fn secondary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => {
            button_style(palette().hover, palette().text, palette().border_strong)
        }
        _ => button_style(palette().raised, palette().text_dim, palette().border),
    }
}

pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered | button::Status::Pressed => button_style(
            palette().accent_hover,
            palette().on_accent,
            palette().accent_hover,
        ),
        button::Status::Disabled => button_style(
            alpha(palette().accent, 0.25),
            alpha(palette().on_accent, 0.6),
            alpha(palette().accent, 0.0),
        ),
        button::Status::Active => {
            button_style(palette().accent, palette().on_accent, palette().accent)
        }
    }
}

/// A tab's label: no surface of its own, the tab provides it.
pub fn tab_label(_: &Theme, status: button::Status) -> button::Style {
    let text = match status {
        button::Status::Hovered | button::Status::Pressed => palette().text_strong,
        _ => palette().text,
    };
    button_style(
        alpha(palette().surface, 0.0),
        text,
        alpha(palette().surface, 0.0),
    )
}

/// The label part of a tag chip: no surface of its own, the chip provides it.
pub fn chip_label(_: &Theme, status: button::Status) -> button::Style {
    let text = match status {
        button::Status::Hovered | button::Status::Pressed => palette().text_strong,
        _ => palette().text,
    };
    button_style(
        alpha(palette().surface, 0.0),
        text,
        alpha(palette().surface, 0.0),
    )
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
            (true, _) => button_style(color, palette().on_accent, color),
            (false, _) if implied => button_style(alpha(color, 0.18), palette().text, color),
            (false, true) => button_style(alpha(color, 0.22), palette().text, alpha(color, 0.0)),
            (false, false) => {
                button_style(alpha(color, 0.0), palette().text_dim, alpha(color, 0.0))
            }
        };
        style.border.radius = Radius::from(4.0);
        style
    }
}

pub fn chip_remove(_: &Theme, status: button::Status) -> button::Style {
    let mut style = match status {
        button::Status::Hovered | button::Status::Pressed => button_style(
            alpha(palette().error, 0.18),
            palette().error,
            alpha(palette().error, 0.0),
        ),
        _ => button_style(
            alpha(palette().error, 0.0),
            palette().text_faint,
            alpha(palette().error, 0.0),
        ),
    };
    style.border.radius = Radius::from(4.0);
    style
}

pub fn swatch(color: Color, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let ring = if selected {
            palette().text_strong
        } else if hovered {
            alpha(palette().text_strong, 0.5)
        } else {
            alpha(color, 0.0)
        };
        button::Style {
            background: Some(color.into()),
            text_color: palette().on_accent,
            border: border(ring, 2.0, 999.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// The "no colour" swatch: an outlined empty circle, ringed like the others when selected.
pub fn swatch_none(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let ring = if selected {
            palette().text_strong
        } else if hovered {
            alpha(palette().text_strong, 0.5)
        } else {
            palette().border_strong
        };
        button::Style {
            background: Some(palette().raised.into()),
            text_color: palette().text_dim,
            border: border(ring, if selected { 2.0 } else { 1.0 }, 999.0),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

// --------------------------------------------------------------------- inputs

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let edge = match status {
        text_input::Status::Focused { .. } => palette().accent,
        text_input::Status::Hovered => palette().border_strong,
        _ => palette().border,
    };
    text_input::Style {
        background: palette().raised.into(),
        border: border(edge, 1.0, 8.0),
        icon: palette().text_dim,
        placeholder: palette().text_faint,
        value: palette().text,
        selection: alpha(palette().accent, 0.35),
    }
}

pub fn check(_: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, hovered) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true),
        checkbox::Status::Disabled { is_checked } => (is_checked, false),
    };
    checkbox::Style {
        background: if checked {
            palette().accent
        } else {
            palette().raised
        }
        .into(),
        icon_color: palette().on_accent,
        border: border(
            if checked {
                palette().accent
            } else if hovered {
                palette().border_strong
            } else {
                palette().border
            },
            1.0,
            4.0,
        ),
        text_color: Some(palette().text),
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
    pub const TO_TOP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 3h14"/><path d="m18 13-6-6-6 6"/><path d="M12 7v14"/></svg>"#;
    pub const TO_BOTTOM: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17V3"/><path d="m6 11 6 6 6-6"/><path d="M19 21H5"/></svg>"#;
    pub const RELOAD: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/></svg>"#;
    pub const COPY: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>"#;
    pub const SUN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>"#;
    pub const MOON: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>"#;
    pub const HOME: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .71-1.53l7-6a2 2 0 0 1 2.58 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>"#;
    pub const SPLIT_RIGHT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/></svg>"#;
    pub const SPLIT_DOWN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 12h18"/></svg>"#;
    pub const CLOSE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#;
    pub const PLUS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>"#;
}
