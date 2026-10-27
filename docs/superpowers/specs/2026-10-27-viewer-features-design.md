# FastLogViewer: navigation, reload, copy, notes, tag stats, columns

Date: 2026-10-27
Applies to: `fastlogviewer` (iced 0.14 UI) and `fastlogviewer-core`.

## Intent

FastLogViewer is a fast, read-only viewer for large logs. This round adds features that help
someone work through a log over time: move around it quickly, notice when it changes, copy text
out, leave notes on it, see how tagged lines are spread through it, and read structured lines as
columns.

The viewer stays read-only. An in-viewer editor (cut, paste, undo, redo) was considered and
**dropped by the user**. Copy is the only clipboard feature.

## Goals

Build these in this order. Each one is independent and gets its own commit.

1. Top and bottom buttons, plus a percent-through-file readout in the status bar.
2. Reload the open file, with an indicator when the file has changed on disk.
3. Copy: selected text, the current line, selected lines, or a range of lines.
4. Tag stats panel.
5. Markup notes on lines and on patterns, saved next to the log.
6. Rendered columns.

## Non-goals / anti-goals

- **No editing of the log file.** The viewer never writes to the log.
- **No live tailing or auto-reload.** Reload only happens when the user asks for it. The viewer
  only *indicates* that the file changed.
- **Notes must never attach to the wrong line.** If a note's line text no longer exists, the note
  is dropped. It is never left pointing at whatever now sits on its old line number.
- **Columns must never mangle lines that don't fit.** A non-matching line renders exactly as it
  does today.
- Nothing may block the UI thread on large files. Work that scans the file runs on the existing
  background-thread + `CancelToken` pattern and fills in progressively.

## Existing pieces to build on

- `LogView` (`src/log_view.rs`) owns scroll position and a **single** selected display row
  (`selected: i64`). Ctrl+Home and Ctrl+End already scroll to the top and bottom.
- `LogFile` (`fastlogviewer-core/src/log_file.rs`) is a read-only mmap plus a line index, held in
  an `Arc`. Reloading means opening a new `LogFile` and swapping it in, the same way `load_file`
  does.
- Patterns: `TemplateIndex` / `LogTemplate` (`templates.rs`) cluster lines into templates with
  variable slots. `LogTemplate::render()` gives the pattern's text. `PrefixShape` already detects
  a leading timestamp (`measure_timestamp`).
- Tags: `TagStore` / `TagDef` (`tags.rs`). Per-tag `MatchIndex` scans are started with
  `App::start_tag_index`. Today they only run for tags that are filtering.
- Status bar: `App::status_bar` / `refresh_status` in `src/app.rs`.

## Feature details

### 1. Top/bottom and percent readout

- Two toolbar buttons, "Top" and "Bottom", that do what Ctrl+Home and Ctrl+End already do. Their
  tooltips show the shortcuts.
- The current line becomes the selected line, so Top selects the first row and Bottom selects the
  last row. Arrow keys continue from there.
- Status bar: show `line N of M (P%)`, where N is the selected line, or the top visible line when
  nothing is selected. With a filter on, N and M count displayed rows, and the actual file line
  number is still shown as it is today.

### 2. Reload

- About every 2 seconds, check the open file's size and last-modified time with `fs::metadata`,
  using an iced `time::every` subscription. Don't read the file contents.
- If either one differs from the values captured at load, show a "File changed" badge in the
  toolbar, styled as a warning. Clicking the badge reloads the file.
- Reload is also available any time from a Reload toolbar button and from **F5**, whether or not
  the file has changed.
- Reload keeps the selected actual line (clamped to the new line count), the scroll position,
  tags, the search, and the wrap state. Pattern results are discarded and the view returns to
  lines; the user presses Ctrl+M again to re-cluster. Columns re-sample automatically if they are
  on. Notes are re-pinned (see §5). *(Claude's call.)*
- If the file has been deleted, the badge says "File missing" and Reload shows an error in the
  status bar. The current view stays open.

### 3. Copy

New selection model in `LogView`:

- **Line selection:** click selects one line, as today. Shift+click or Shift+arrow extends the
  selection to a contiguous block of lines (anchor plus active end). Ctrl+A selects all
  displayed lines.
- **Text selection:** click-drag inside a line selects characters. A drag that crosses lines
  selects from the start point to the end point across those lines, like a normal text view.

Copy actions, from a Copy toolbar button with a dropdown and from the keyboard:

- **Ctrl+C:** copies the text selection if there is one. Otherwise it copies the selected
  line(s).
- **Copy line:** copies the current line.
- **Copy selected lines:** copies all lines in the selection.
- **Copy range…:** opens a small prompt, like the existing go-to-line prompt, that takes `from-to`
  file line numbers and copies those lines.

Rules:

- With a filter on, copying selected lines copies only the displayed lines. Copy range copies
  actual file lines, including hidden ones.
- Copied lines are joined with `\n`, and line numbers are not included.
- Use iced's `clipboard::write`.
- Copying a very large range, such as Ctrl+A on a multi-GB file, must not freeze the UI. Builder's
  call: cap the copy at a size, for example 256 MB, and say so in the status bar, or build the
  string on a background thread.

### 4. Tag stats panel

- A "Stats" button in the tag bar opens a side panel inside the main window, docked on the right.
  It is not a separate OS window.
- Opening the panel starts `MatchIndex` scans for every tag that doesn't already have one. The
  panel fills in as the scans progress and shows a progress indicator while they run. Closing
  the panel stops scans that were started only for stats. Track stats-only scans separately
  from filter scans, because `sync_tag_filters` stops any scan whose tag isn't filtering.
- One row per tag, in tag-bar order, using the tag's color:
  - **Count:** number of matching lines.
  - **Share:** percent of all lines. When a filter is on, also show the share among displayed
    lines: the tag's matches that are currently displayed, divided by the number of displayed
    lines. This uses `IntersectionIndex` on a background thread and is recomputed when the
    filter or search changes. *(Claude's call.)*
  - **Distribution strip:** a thin horizontal bar covering the whole file, split into buckets
    (for example 200). Each bucket is shaded by match density. Clicking a bucket jumps to the
    first match in it.
  - **First and last:** the line numbers of the first and last matches, each clickable to jump to
    that line. Each also shows the line's timestamp, from `PrefixShape::measure_timestamp`, when
    one is detected.
- Severity-level tags with a threshold count only their own pattern's matches, not everything at
  or above the threshold.
- Stats update when tags change or the file is reloaded.

### 5. Markup notes

**What can have a note**

- **A line note** belongs to one log line. Add it with Ctrl+Shift+N on the selected line, or with
  a "+" that appears in the gutter when the pointer is over a line. No context menu exists today,
  and this doesn't add one. *(Claude's call.)*
- **A pattern note** belongs to a pattern found by the Patterns view. Add it from that pattern's
  row in the Patterns view. It shows on every line that belongs to the pattern.

**Indicator and sidebar**

- Lines with a note, either a line note or a pattern note, show a small marker in the gutter.
  Pattern-note markers look slightly different from line-note markers.
- Clicking a marker opens a notes sidebar docked on the right, sharing space with the stats
  panel; only one panel is open at a time. The sidebar shows that line's line note and any
  pattern note, and each can be edited or deleted there.
- The sidebar also lists every note in the file. Clicking one jumps to its line.

**Pinning: notes are tied to text, not to line numbers**

- A line note stores the line's exact text and the line number where it was created.
- On load and on reload, look for that text at the stored line number. If it isn't there, use the
  nearest line, by line distance, whose text is identical. If no line has that text, **drop the
  note** and update the saved file.
- A pattern note stores the pattern's rendered text (`LogTemplate::render()`). It is matched to
  the pattern with identical text. Pattern notes only show once clustering has run. Clustering
  can be partial (filtered scope) and its text can drift, so an unmatched pattern note is kept
  and listed in the sidebar as "not matched in this run", where the user can delete it. It is
  only dropped automatically after a complete, unfiltered clustering of the whole file still
  finds no matching pattern. *(Claude's call. Proposed to the user, and the user went ahead
  without objecting.)*
- Searching for re-pinned lines must run in the background on large files. Notes appear when
  that search finishes.

**Storage**

- Notes are saved in `<full log file name>_markup.json`, in the same directory as the log. For
  example, `app.log` gets `app.log_markup.json`. The full name, extension included, keeps
  `app.log` and `app.txt` from sharing a notes file. The JSON also records the log's file name,
  and a notes file whose recorded name doesn't match is ignored and never rewritten.
  *(Claude's call.)*
- The file is written automatically on every change; there is no Save button.
- If the file would be empty, delete it.
- If the directory is read-only, show an error in the status bar and keep the notes in memory.
- The JSON format is the builder's call. It needs a version field, a list of line notes (text,
  line, note, created and updated times), and a list of pattern notes (pattern text, note,
  times).

### 6. Columns

- A "Columns" toolbar toggle, like Wrap and Patterns. It is off by default.
- When turned on, use the template clustering to find the dominant line shape, meaning the most
  common token layout among the top templates. Split it into columns: the timestamp prefix if one
  is detected, each slot or fixed field, and the message as the last column, which takes the
  remaining width.
- Lines that match the shape render in aligned columns, with a header row pinned at the top.
  Headers use obvious names where possible ("time", "level") and `col N` otherwise.
- Lines that don't match the shape render as plain full-width text, exactly as they do today.
- Search, tags, filters, selection, copy (which copies the raw line), and notes keep working in
  column mode.
- Columns run their own clustering over a sample of lines, separate from the Patterns view's
  `TemplateIndex`, and never affect note matching. *(Claude's call.)*
- The heuristic for finding the dominant shape and widths is the builder's call. Column widths
  should be based on a sample, not a full scan, and must not jitter while scrolling.

## Constraints

- Rust, iced 0.14, Windows-first. Follow existing style, and keep `rustfmt` clean.
- Commit each feature separately.
- Core logic that needs no UI (pinning notes, the stats bucketing, column shape detection,
  parsing the range prompt) goes in `fastlogviewer-core` with unit tests.

## Left to the builder

The exact icons and layout, the shortcut for adding a note, the copy-size safeguard, the bucket
count, the JSON schema details, and the column-shape heuristic.
