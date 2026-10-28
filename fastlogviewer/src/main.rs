// FastLogViewer — native Windows log viewer for multi-GB text files.
//
// Architecture (this is the whole trick):
//   1. Memory-map the file        -> zero copy, zero load time, OS pages it in on demand.
//   2. Index line starts in bg    -> SWAR scan for '\n', ~2-4 GB/s, no allocations per line.
//   3. Index matches in bg        -> ONE scan feeds highlight + F3 + filter. UI never blocks.
//   4. Draw only visible lines    -> one canvas, ~60 text runs per frame, <1 ms.
//
// Steps 1-3 live in the `fastlogviewer-core` crate; this one is step 4 and the window around it.
//
// Build:  cargo build --release
//
// Keys: Ctrl+O open · Ctrl+F focus search · F3 / Shift+F3 next/prev match
//       Ctrl+H toggle filter · Ctrl+G goto line · Ctrl+C copy line · Ctrl+M patterns
//       Esc clear search / step out one zoom level

// Detach the console on release builds so launching from Explorer doesn't flash a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod format;
mod log_view;
mod notes_ui;
mod stats_ui;
mod tag_ui;
mod theme;
mod win;

use iced::{
    window::{self, settings::PlatformSpecific},
    Size,
};

fn main() -> iced::Result {
    let initial_file = std::env::args().nth(1);

    let settings = window::Settings {
        size: Size::new(1180.0, 740.0),
        min_size: Some(Size::new(560.0, 260.0)),
        icon: win::load_dll_icon("C:/Windows/System32/SHELL32.dll", 260),
        platform_specific: PlatformSpecific {
            corner_preference: window::settings::platform::CornerPreference::RoundSmall,
            ..PlatformSpecific::default()
        },
        ..window::Settings::default()
    };

    iced::application(
        move || app::App::new(initial_file.clone()),
        app::App::update,
        app::App::view,
    )
    .title(app::App::title)
    .subscription(app::App::subscription)
    .theme(|_: &app::App| theme::app_theme())
    .font(theme::JETBRAINS_MONO)
    .font(theme::IBM_PLEX)
    .default_font(theme::UI)
    .antialiasing(true)
    .window(settings)
    .run()
}
