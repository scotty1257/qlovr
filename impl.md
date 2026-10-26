# FastLogViewer — Implementation Progress

_Last updated: 2026-10-26_

FastLogViewer is a native Windows viewer for multi-GB text logs. It is a Rust port of an earlier
C# WinForms tool, split into two crates:

| Crate | Role | Status |
|---|---|---|
| `fastlogviewer-core` | Memory-mapping, line indexing, search, filtering, tags, Drain-style pattern clustering | Stable, unchanged by the UI work |
| `fastlogviewer` | The window: toolbar, tag row, log surface, status bar, dialogs | Rewritten on iced 0.14 (commit `3c0a782`) |

## How it works

1. **Memory-map the file.** No copying and no load time; the OS pages the file in on demand.
2. **Index line starts on a background thread** with a SWAR scan for `\n` (~2–4 GB/s).
3. **Index matches on a background thread.** One scan feeds highlighting, F3 navigation and the
   filter, so the UI never blocks.
4. **Draw only the visible rows**, about 60 text runs per frame, so a huge file scrolls like a
   small one.

Workers share the file through `Arc<LogFile>` and publish progress through atomics. The UI polls
them with a 50 ms tick, but only while something is still running.

## Features

- **Search:** plain text or regex, with a case toggle, live match count and F3 / Shift+F3 to step
  between matches (wraps at either end). Typing is debounced by 250 ms.
- **Filter (Ctrl+H):** shows only matching lines. It combines with tag filters as an intersection.
- **Tags:** saved patterns, each with its own colour, that can highlight matches, filter lines, or
  both. Stored in `%APPDATA%\FastLogViewer\tags.json`.
- **Patterns (Ctrl+M):** semantic zoom. Lines are clustered into templates; Enter or a double-click
  zooms into one template's lines; Esc steps back out a level. While zoomed in, the search box also
  accepts slot queries such as `slot2 > 2000 AND slot1 = warn`.
- **Other:** go to line (Ctrl+G), copy the selected line (Ctrl+C), open (Ctrl+O), drag-and-drop to
  open, and the file path as a command-line argument.

## UI rewrite: egui → iced (done)

### Module layout (`fastlogviewer/src`)

| File | Contents |
|---|---|
| `main.rs` | iced application setup: fonts, theme, window settings, icon |
| `app.rs` | `App` state, `update` / `view` / `subscription`, worker orchestration, filter composition |
| `log_view.rs` | `LogView` state plus the canvas `Program` that draws rows, highlights and scrollbars |
| `tag_ui.rs` | Tag chips, the tag editor dialog, the go-to-line dialog, and modal/tooltip helpers |
| `theme.rs` | Colour palette, fonts, every widget style, inline SVG icons |
| `format.rs` | Thousands-separator number formatting |
| `win.rs` | Win32 helpers: dark title bar tinted to match the toolbar, rounded corners, shell32 icon |

### Design decisions

- **The canvas can't change the view directly.** iced draws through `&self`, so mouse input on
  the log becomes a `ViewMessage` that `App` applies. The one exception is the viewport size, which
  `draw` writes into a `Cell` because keyboard paging needs it and only layout knows it.
- **Keyboard routing.** All key presses arrive through `event::listen_with`. Global shortcuts are
  handled first. Keys a focused text input already consumed (`Status::Captured`) never reach the
  log, which matches the WinForms split between `ProcessCmdKey` and `LogView.OnKeyDown`.
- **Character width is fixed at `0.6 × font size`**, JetBrains Mono's real advance width.
  Highlights are placed by column, so this must be exact.
- **Each line is drawn as runs of a single colour** (timestamp, level, matches, plain text), so no
  glyph is ever drawn twice.
- **The title bar is tinted on the first "window opened" or "window focused" event.** The
  window-open event alone can fire before the subscription that would catch it exists.
- **The UI crate needs Rust 1.88** (iced 0.14's minimum); the core crate keeps the workspace's
  1.85.

### Visual design

- Dark graphite theme with one blue accent; amber for search matches.
- ERROR, WARN, INFO and DEBUG words are coloured, and errors and warnings get a tick in the
  line-number column. Leading timestamps are dimmed. The row under the mouse is shaded.
- Rounded match highlights, and a tick on the scrollbar for each match.
- Search box with an icon and the match count inside it, next to a `.*` / `Aa` / `Filter` button
  group and a Patterns button.
- Tag chips tinted in each tag's colour, a strip naming the current zoom level, keycap-style
  shortcut hints, a screen for when no file is open, and card dialogs over a dimmed background.

## Verification

| Check | Result |
|---|---|
| `cargo test` (core suites) | All pass |
| `cargo build` / `cargo build --release` | Clean, no warnings |
| `cargo clippy -p fastlogviewer` | One `manual_is_multiple_of` hint in `format.rs` (code moved over unchanged) |
| Manual run on `sample.log` (200k lines, 11.3 MB) | Viewing, search + F3, filter, patterns mode, tag create + highlight, tag dialog, empty state, modal click-blocking, release build |

**Not yet tested by hand:** the Ctrl+O file dialog, drag-and-drop, the go-to-line dialog, dragging
the scrollbar thumb, and very large (multi-GB) files with the new renderer.

## Known issues

1. **The search bar's case toggle is inverted.** `App::start_match_scan` passes `case_on` ("Match
   case") into `Matcher::new`'s `ignore_case` parameter. This was inherited from the egui version
   and is not yet fixed; it's a one-line change.
2. **The repo screenshots are out of date.** `shot.png`, `shot_f3.png`, `shot_search.png` and
   `shot_templates.png` still show the old egui UI.
3. **The file dialog blocks.** `rfd::FileDialog` runs synchronously inside `update`, as it did
   under egui. It works on Windows, but the event loop pauses while the dialog is open.
4. **Tab doesn't move focus.** iced has no Tab-key focus navigation for buttons, so the dialogs
   are mouse- and Enter-driven.

## Possible next steps

- Fix the case-toggle inversion (known issue 1).
- Replace the stale screenshots.
- Test the remaining features by hand, especially multi-GB files and scrollbar dragging.
- Optionally: a light theme or following the system theme (all colours live in `theme.rs`), and
  an async file dialog (`rfd::AsyncFileDialog` with `Task::perform`).
