// FastLogViewer — native Windows log viewer for multi-GB text files.
//
// Architecture (this is the whole trick):
//   1. Memory-map the file        -> zero copy, zero load time, OS pages it in on demand.
//   2. Index line starts in bg    -> SWAR scan for '\n', ~2-4 GB/s, no allocations per line.
//   3. Index matches in bg        -> ONE scan feeds highlight + F3 + filter. UI never blocks.
//   4. Draw only visible lines    -> ~60 painter calls per frame, <1 ms.
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
mod log_view;
mod tag_ui;
mod theme;
mod widgets;
mod win;

use std::sync::Arc;

use eframe::egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    let initial_file = std::env::args().nth(1);

    let mut viewport = ViewportBuilder::default()
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([560.0, 260.0])
        .with_title("FastLogViewer")
        .with_drag_and_drop(true);

    // The same shell32 icon the C# build pulled in; a failure here just means no custom icon.
    if let Some(icon) = win::load_dll_icon("C:/Windows/System32/SHELL32.dll", 260) {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "FastLogViewer",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::App::new(&cc.egui_ctx, initial_file)) as Box<dyn eframe::App>)
        }),
    )
}
