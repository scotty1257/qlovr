//! One tab: a log's toolbar, tag row, log surface, status strip, and the worker threads that feed
//! them. The window around the tabs (panes, tab strips, window chrome) lives in `app.rs`.
//!
//! One structural difference from the C# `MainForm` worth naming. The original kept a `_stale` list
//! of worker threads and joined them before `Dispose`ing the memory map, because a scan still
//! running on a freed view would crash — and when a worker refused to stop in time it chose to leak
//! the mapping instead. Here every worker holds an `Arc<LogFile>`, so the mapping simply outlives
//! the last reader and unmaps itself. Cancellation stays (it stops useless work); the join
//! bookkeeping is gone because the hazard it guarded against cannot occur.
//!
//! Workers publish progress through atomics, not messages, so while anything is in flight a 50 ms
//! tick re-reads them; the subscription drops the tick once everything has settled.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use fastlogviewer_core::{
    bucket_counts, bucket_start, copy_text_of, detect_columns, diff_logs, intersection_count,
    join_lines, load_notes, notes_path, parse_line_range, pin_line_notes, save_notes, CancelToken,
    ChangeSet, ColumnFitIndex, ColumnSettings, ColumnShape, ComplementIndex, FilterIndex,
    IntersectionIndex, Joined, LineDiff, LineNote, LoadedNotes, LogFile, LogTemplate, LogViewMode,
    MatchIndex, Matcher, NotesFile, PatternNote, PrefixShape, RecentFiles, SlotFilterIndex,
    SlotQuery, TagDef, TagStore, TemplateIndex, UnionFilterIndex, COPY_LIMIT_BYTES, TAG_PALETTE,
};
use iced::keyboard::{self, key::Named, Key};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, canvas, column, container, operation, responsive, row, scrollable, space, stack, svg,
    text, text_editor, text_input, tooltip, Id,
};
use iced::{Alignment, Color, Element, Length, Padding, Task, Widget};
use uuid::Uuid;

use crate::format::{thousands, thousands_1dp};
use crate::log_view::{LogView, ViewMessage};
use crate::notes_ui::{self, notes_panel, LineRow, NotesMessage, PatternRow};
use crate::stats_ui::{stats_panel, StatsMessage, TagStat, STRIP_BUCKETS};
use crate::tag_ui::{
    self, copy_view, goto_view, modal, tag_chip, with_tip, ChipAction, ColumnsDialog,
    ColumnsMessage, CopyChoices, CopyMessage, GotoMessage, PopupMessage, PopupOutcome,
    TagEditPopup,
};
use crate::theme::{self, icons};

const HINTS: [(&str, &str); 5] = [
    ("Ctrl O", "open"),
    ("Ctrl F", "search"),
    ("F3", "next"),
    ("Ctrl H", "filter"),
    ("Ctrl M", "patterns"),
];

/// Below this width the toolbar puts the search box on a row of its own.
const NARROW_TOOLBAR: f32 = 1000.0;
/// How long a status-bar notice (a failed reload, a truncated copy) stays up.
const NOTICE_FOR: Duration = Duration::from_secs(6);

/// What `fs::metadata` said about the file when it was loaded; a difference means it changed.
#[derive(PartialEq, Clone, Copy)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(path: &Path) -> Option<FileStamp> {
        let meta = std::fs::metadata(path).ok()?;
        Some(FileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum DiskState {
    Unchanged,
    Changed,
    Missing,
}

/// A short message in the status bar: a failed reload, a copy's result.
struct Notice {
    text: String,
    error: bool,
    at: Instant,
}

#[derive(Copy, Clone)]
enum CopyKind {
    /// The dragged-over text, or the selected lines when nothing was dragged (Ctrl+C).
    Selection,
    Line,
    Lines,
    /// 0-based, inclusive file lines.
    Range(usize, usize),
}

/// The panel docked to the right of the log, if any.
#[derive(PartialEq, Eq, Copy, Clone)]
enum SidePanel {
    Stats,
    Notes,
}

/// Line notes being matched to the file's text on a worker; `None` until done.
struct NotesPin {
    result: Arc<Mutex<Option<Vec<Option<usize>>>>>,
    ct: CancelToken,
}

/// What the notes panel's editors are open on.
struct NoteFocus {
    /// 0-based real line.
    line: Option<i64>,
    /// The pattern's text.
    pattern: Option<String>,
}

/// A match scan the stats panel started for a tag that isn't filtering (filtering tags already
/// have one in `tag_indexes`). The matcher tells whether the tag was edited since.
struct StatsScan {
    matcher: Arc<Matcher>,
    index: Arc<MatchIndex>,
    ct: CancelToken,
}

/// How many of a tag's matches are among the displayed lines, counted on a worker. `key` names
/// the inputs it was counted from: the filter, the tag's index and both their sizes.
struct ShownCount {
    key: [usize; 4],
    /// -1 until counted.
    count: Arc<AtomicI64>,
    ct: CancelToken,
}

/// A reload's comparison with the version before it, running on a worker; `None` until done,
/// then `Some(None)` when nothing had changed.
struct DiffJob {
    /// The version compared against, kept so a reload that lands mid-comparison still compares
    /// from it and no change is skipped.
    old: Arc<LogFile>,
    result: Arc<Mutex<Option<Option<LineDiff>>>>,
    ct: CancelToken,
}

/// Where the view was before a reload, by real line number, put back once the new index is done.
#[derive(Copy, Clone)]
struct Restore {
    selected: i64,
    top: i64,
}

/// Which index the ad-hoc/tag composition settled on, so the zoom layer can ask "is my base ready"
/// the way the C# did by comparing object references.
#[derive(PartialEq, Eq, Copy, Clone)]
enum Base {
    None,
    Intersection,
    AdHoc,
    TagUnion,
}

#[derive(Debug, Clone)]
pub enum Message {
    SearchChanged(String),
    SearchSubmitted,
    ToggleRegex,
    ToggleCase,
    ToggleFilter,
    ToggleExclude,
    TogglePatterns,
    ToggleWrap,
    ToggleColumns,
    Columns(ColumnsMessage),
    GoTop,
    GoBottom,
    OpenDialog,
    OpenRecent(PathBuf),
    /// Opens the file dialog in one of the common log folders.
    BrowseFolder(PathBuf),
    Reload,
    CheckDisk,
    /// Selects the first line of the next block that changed on disk.
    NextChange,
    ToggleChangesFilter,
    ClearChanges,
    NewTag,
    Chip(Uuid, ChipAction),
    Popup(PopupMessage),
    Goto(GotoMessage),
    OpenCopy,
    Copy(CopyMessage),
    ToggleStats,
    Stats(StatsMessage),
    ToggleNotes,
    Notes(NotesMessage),
    View(ViewMessage),
    KeyPressed {
        key: Key,
        modifiers: keyboard::Modifiers,
        /// A focused text input already handled it.
        captured: bool,
    },
    Tick,
    /// Handled by the window, which retints the title bar; the toolbar only raises it.
    ToggleTheme,
}

pub struct Tab {
    view: LogView,
    file: Option<Arc<LogFile>>,
    file_ct: Option<CancelToken>,
    title: String,
    file_stamp: Option<FileStamp>,
    disk_state: DiskState,
    pending_restore: Option<Restore>,
    notice: Option<Notice>,
    /// Lines changed on disk since the file was opened or the marks were cleared.
    changes: Arc<ChangeSet>,
    /// Show only the changed lines.
    changes_filter: bool,
    diff_job: Option<DiffJob>,

    search: String,
    regex_on: bool,
    case_on: bool,
    filter_on: bool,
    /// Filter shows the lines that do *not* match.
    exclude_on: bool,
    count_text: String,
    count_error: bool,

    matcher: Option<Arc<Matcher>>,
    matches: Option<Arc<MatchIndex>>,
    match_ct: Option<CancelToken>,

    tag_store: TagStore,
    tag_indexes: HashMap<Uuid, Arc<MatchIndex>>,
    tag_cts: HashMap<Uuid, CancelToken>,
    tag_union: Option<Arc<UnionFilterIndex>>,
    union_ct: Option<CancelToken>,
    last_adhoc_complete: bool,
    last_union_complete: bool,
    last_slot_complete: bool,

    templates: Option<Arc<TemplateIndex>>,
    template_ct: Option<CancelToken>,
    zoom: Option<Arc<LogTemplate>>,
    slot_filter: Option<Arc<SlotFilterIndex>>,
    slot_ct: Option<CancelToken>,
    slot_error: Option<String>,

    status_left: String,
    busy: bool,
    type_deadline: Option<Instant>,
    modifiers: keyboard::Modifiers,
    popup: Option<TagEditPopup>,
    goto_open: bool,
    goto_text: String,
    copy_open: bool,
    copy_range: String,
    copy_error: Option<String>,

    side_panel: Option<SidePanel>,
    stats_scans: HashMap<Uuid, StatsScan>,
    stats_shown: HashMap<Uuid, ShownCount>,
    /// The file's timestamp shape, sniffed once for the stats panel's first/last times.
    stats_prefix: Option<PrefixShape>,

    notes: NotesFile,
    /// Why notes can't be saved (the markup file belongs to another log), if they can't.
    notes_read_only: Option<String>,
    notes_pin: Option<NotesPin>,
    /// The line notes have been matched to this file's text and can be shown and edited.
    notes_placed: bool,
    note_focus: Option<NoteFocus>,
    line_editor: text_editor::Content,
    pattern_editor: text_editor::Content,
    /// The pattern each pattern note matched in the last finished clustering, in note order.
    pattern_matches: Vec<Option<Arc<LogTemplate>>>,
    /// Which clustering `pattern_matches` came from, by address.
    pattern_matches_from: usize,
    /// The last clustering covered the whole file, not just a filtered view of it.
    templates_whole_file: bool,

    /// Columns is switched on; the layout is found once the line index is complete.
    columns_wanted: bool,
    /// Which file the column layout was found for, by address.
    columns_for: usize,
    column_settings: ColumnSettings,
    columns_dialog: Option<ColumnsDialog>,
    /// The lines that fit the column layout, while lines that don't are hidden.
    column_fit: Option<Arc<ColumnFitIndex>>,
    column_fit_ct: Option<CancelToken>,
    last_fit_complete: bool,

    /// Each tab's search box needs its own id, or Ctrl+F would focus every pane's.
    search_input: Id,
    /// A file picked while this tab already shows one; the window opens it in a new tab.
    open_request: Option<PathBuf>,
    /// The tags were edited here and the other tabs should pick the change up.
    tags_changed: bool,

    recent: RecentFiles,
    /// Common log folders that exist on this machine, found once at startup.
    log_folders: Vec<(&'static str, PathBuf)>,
}

impl Tab {
    pub fn new() -> Self {
        let mut tab = Tab {
            view: LogView::new(),
            file: None,
            file_ct: None,
            title: "FastLogViewer".to_string(),
            file_stamp: None,
            disk_state: DiskState::Unchanged,
            pending_restore: None,
            notice: None,
            changes: Arc::default(),
            changes_filter: false,
            diff_job: None,
            search: String::new(),
            regex_on: false,
            case_on: false,
            filter_on: false,
            exclude_on: false,
            count_text: String::new(),
            count_error: false,
            matcher: None,
            matches: None,
            match_ct: None,
            tag_store: TagStore::new(),
            tag_indexes: HashMap::new(),
            tag_cts: HashMap::new(),
            tag_union: None,
            union_ct: None,
            last_adhoc_complete: false,
            last_union_complete: false,
            last_slot_complete: false,
            templates: None,
            template_ct: None,
            zoom: None,
            slot_filter: None,
            slot_ct: None,
            slot_error: None,
            status_left: "Open a log file to begin".to_string(),
            busy: false,
            type_deadline: None,
            modifiers: keyboard::Modifiers::default(),
            popup: None,
            goto_open: false,
            goto_text: String::new(),
            copy_open: false,
            copy_range: String::new(),
            copy_error: None,
            side_panel: None,
            stats_scans: HashMap::new(),
            stats_shown: HashMap::new(),
            stats_prefix: None,
            notes: NotesFile::for_log(Path::new("")),
            notes_read_only: None,
            notes_pin: None,
            notes_placed: false,
            note_focus: None,
            line_editor: text_editor::Content::new(),
            pattern_editor: text_editor::Content::new(),
            pattern_matches: Vec::new(),
            pattern_matches_from: 0,
            templates_whole_file: false,
            columns_wanted: false,
            columns_for: 0,
            column_settings: ColumnSettings::load(),
            columns_dialog: None,
            column_fit: None,
            column_fit_ct: None,
            last_fit_complete: false,
            search_input: Id::unique(),
            open_request: None,
            tags_changed: false,
            recent: RecentFiles::load(),
            log_folders: log_folders(),
        };

        tab.tag_store.load();
        tab.sync_view_tags();
        tab.refresh_status();
        tab
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    /// The file's name for the tab strip, or "New tab" on the start screen.
    pub fn label(&self) -> String {
        self.file
            .as_ref()
            .and_then(|f| f.path().file_name())
            .map_or_else(
                || "New tab".to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file.as_ref().map(|f| f.path())
    }

    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }

    /// Workers are running or typing is being debounced, so the 50 ms tick is needed.
    pub fn needs_tick(&self) -> bool {
        self.busy || self.type_deadline.is_some()
    }

    pub fn set_modifiers(&mut self, modifiers: keyboard::Modifiers) {
        self.modifiers = modifiers;
    }

    /// Stops the tab's workers when it is closed; dropping the tab alone would leave them running.
    pub fn close(&mut self) {
        self.release_current();
    }

    pub fn take_open_request(&mut self) -> Option<PathBuf> {
        self.open_request.take()
    }

    pub fn take_tags_changed(&mut self) -> bool {
        std::mem::take(&mut self.tags_changed)
    }

    pub fn tags(&self) -> &[TagDef] {
        &self.tag_store.tags
    }

    /// Takes on tags edited in another tab. Their patterns may have changed, so every running tag
    /// index is restarted rather than trusted.
    pub fn adopt_tags(&mut self, tags: &[TagDef]) {
        self.tag_store.tags = tags.to_vec();
        self.cancel_all_tag_filters();
        self.sync_tag_filters();
        self.rebuild_union();
        self.sync_view_tags();
    }

    /// Focuses the search box (Ctrl+F).
    pub fn focus_search(&self) -> Task<Message> {
        operation::focus(self.search_input.clone())
            .chain(operation::text_input::select_all(self.search_input.clone()))
    }

    fn sync_view_tags(&mut self) {
        self.view.set_tags(self.tag_store.tags.clone());
    }

    // ------------------------------------------------------------ file loading

    pub fn load_file(&mut self, path: &str) {
        self.release_current();
        self.changes = Arc::default();
        self.changes_filter = false;
        self.pending_restore = None;
        self.notice = None;
        self.disk_state = DiskState::Unchanged;
        // Taken before opening, so a write that lands in between shows as a change, not a miss.
        self.file_stamp = FileStamp::of(Path::new(path));

        let absolute = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
        let file = match LogFile::open(path) {
            Ok(f) => Arc::new(f),
            Err(e) => {
                // A recent file that no longer opens would only fail again.
                self.recent.remove(&absolute);
                self.recent.save();
                rfd::MessageDialog::new()
                    .set_title("Cannot open file")
                    .set_description(e.to_string())
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                return;
            }
        };

        self.recent.add(&absolute);
        self.recent.save();

        self.file = Some(file.clone());
        self.view.set_file(Some(file.clone()));
        self.title = format!(
            "{}  —  FastLogViewer",
            Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );

        let ct = CancelToken::new();
        self.file_ct = Some(ct.clone());
        let indexing = file.clone();
        std::thread::spawn(move || indexing.build_index(&ct));
        self.load_notes(&file);

        self.start_match_scan();
        self.view.set_active_filter(None);
        self.sync_tag_filters();
    }

    /// Re-opens the same file, keeping tags, the search, wrapping, and the selected and top lines.
    /// Pattern results are dropped; the view goes back to lines.
    fn reload(&mut self) {
        let Some(file) = self.file.clone() else {
            return;
        };
        let path = file.path().to_path_buf();
        if !path.is_file() {
            self.disk_state = DiskState::Missing;
            self.notify("Cannot reload: the file no longer exists", true);
            return;
        }

        let restore = Restore {
            selected: self.view.selected_actual_line(),
            top: self.view.top_line(),
        };
        let base = self.diff_job.as_ref().map_or(file, |job| job.old.clone());
        let (changes, changes_filter) = (self.changes.clone(), self.changes_filter);
        // In patterns, or zoomed into one, the search box holds a pattern or slot filter that
        // means nothing to plain lines.
        if self.view.mode() == LogViewMode::Templates || self.zoom.is_some() {
            self.search.clear();
        }
        self.load_file(&path.to_string_lossy());
        if let Some(new) = self.file.clone() {
            self.pending_restore = Some(restore);
            // The marks are in the old version's numbering; they come back once the comparison
            // has renumbered them.
            self.changes = changes;
            self.changes_filter = changes_filter;
            self.start_diff(base, new);
        }
    }

    /// Compares the reloaded file with the version before it, once its line index is done.
    fn start_diff(&mut self, old: Arc<LogFile>, new: Arc<LogFile>) {
        let result = Arc::new(Mutex::new(None));
        let ct = CancelToken::new();
        self.diff_job = Some(DiffJob {
            old: old.clone(),
            result: result.clone(),
            ct: ct.clone(),
        });
        std::thread::spawn(move || {
            while !new.index_complete() {
                if ct.is_cancelled() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let diff = diff_logs(&old, &new, &ct);
            if !ct.is_cancelled() {
                *result.lock().unwrap() = Some(diff);
            }
        });
    }

    /// Folds a finished comparison into the change marks.
    fn sync_diff(&mut self) {
        let Some(job) = &self.diff_job else {
            return;
        };
        let Some(diff) = job.result.lock().unwrap().take() else {
            return;
        };
        self.diff_job = None;
        if let Some(diff) = diff {
            self.changes = Arc::new(self.changes.with(diff));
        }
        self.show_changes();
    }

    fn show_changes(&mut self) {
        self.view
            .set_changes((!self.changes.is_empty()).then(|| self.changes.clone()));
        if self.changes_filter {
            self.recompute_active_filter();
        }
    }

    fn next_change(&mut self) {
        if let Some(line) = self.changes.next_after(self.view.selected_actual_line()) {
            self.view.go_to_actual(line as i64);
        }
    }

    /// The file changed on disk and can be reloaded without disturbing what the user is doing:
    /// the last load is fully indexed, no comparison is pending, and the view isn't showing
    /// patterns (a reload drops them) or a dialog.
    fn can_auto_reload(&self) -> bool {
        self.file.as_ref().is_some_and(|f| f.index_complete())
            && self.diff_job.is_none()
            && self.view.mode() == LogViewMode::Lines
            && self.zoom.is_none()
            && self.popup.is_none()
            && !self.goto_open
            && !self.copy_open
    }

    fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    fn check_disk(&mut self) {
        let Some(file) = &self.file else {
            return;
        };
        self.disk_state = match FileStamp::of(file.path()) {
            None => DiskState::Missing,
            Some(stamp) if Some(stamp) == self.file_stamp => DiskState::Unchanged,
            Some(_) => DiskState::Changed,
        };
        if self.disk_state == DiskState::Changed && self.can_auto_reload() {
            self.reload();
        }
    }

    /// Puts the view back where it was before a reload, once the line index is complete.
    fn apply_pending_restore(&mut self, file: &LogFile) {
        let Some(restore) = self.pending_restore.take() else {
            return;
        };
        let last = file.line_count() as i64 - 1;
        if last < 0 {
            return;
        }
        if restore.top >= 0 && !self.view.filter_on() {
            self.view.scroll_to(restore.top.min(last));
        }
        if restore.selected >= 0 {
            self.view.go_to_actual(restore.selected.min(last));
        }
    }

    /// Cancels every worker and drops our handle on the mapping. Unlike the C#, nothing has to be
    /// joined first: the mapping lives until the last worker's `Arc` goes away.
    fn release_current(&mut self) {
        if let Some(job) = self.diff_job.take() {
            job.ct.cancel();
        }
        self.view.set_changes(None);
        self.cancel_match_scan();
        self.cancel_all_tag_filters();
        self.stop_stats();
        self.release_notes();
        self.view.set_columns(None);
        self.columns_for = 0;
        self.cancel_column_fit();
        self.cancel_clustering();
        self.cancel_slot_filter();
        self.zoom = None;
        self.slot_error = None;
        self.view.show_lines();
        if let Some(ct) = &self.file_ct {
            ct.cancel();
        }
        self.view.set_file(None);
        self.file = None;
        self.file_ct = None;
        self.matches = None;
        self.title = "FastLogViewer".to_string();
    }

    // -------------------------------------------------------------- ad-hoc scan

    fn cancel_match_scan(&mut self) {
        if let Some(ct) = self.match_ct.take() {
            ct.cancel();
        }
    }

    fn start_match_scan(&mut self) {
        self.type_deadline = None;

        // The one search box serves three jobs; mode picks which.
        if self.view.mode() == LogViewMode::Templates {
            self.show_template_list();
            return;
        }
        if self.zoom.is_some() && SlotQuery::looks_like_slot_query(&self.search) {
            self.start_slot_filter();
            return;
        }
        if self.zoom.is_some() && self.slot_filter.is_some() {
            self.cancel_slot_filter();
        }

        self.cancel_match_scan();
        let Some(file) = self.file.clone() else {
            return;
        };

        let matcher = Arc::new(Matcher::new(&self.search, self.regex_on, self.case_on));
        self.matcher = Some(matcher.clone());

        if !matcher.is_valid() {
            self.matches = None;
            self.view.set_matches(Some(matcher), None);
            self.count_error = true;
            self.count_text = "invalid regex".to_string();
            self.last_adhoc_complete = false;
            self.recompute_active_filter();
            return;
        }

        self.count_error = false;
        self.matches = if matcher.is_empty() {
            None
        } else {
            Some(Arc::new(MatchIndex::new(matcher.clone(), file.length())))
        };
        self.last_adhoc_complete = false;
        self.view.set_matches(Some(matcher), self.matches.clone());

        let Some(index) = self.matches.clone() else {
            self.count_text.clear();
            self.recompute_active_filter();
            return;
        };

        let ct = CancelToken::new();
        self.match_ct = Some(ct.clone());
        std::thread::spawn(move || index.build(&file, &ct));
        self.recompute_active_filter();
    }

    // ----------------------------------------------------------- tag filtering

    /// Runs an index for exactly the tags the filter includes (see `TagStore::filtering_ids`),
    /// rebuilding the union if that changed anything.
    fn sync_tag_filters(&mut self) {
        let wanted = self.tag_store.filtering_ids();
        let stale: Vec<Uuid> = self
            .tag_indexes
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect();
        let mut changed = !stale.is_empty();
        for id in stale {
            self.stop_tag_filter(id);
        }
        for id in wanted {
            if !self.tag_indexes.contains_key(&id) {
                changed |= self.start_tag_index(id);
            }
        }
        if changed {
            self.rebuild_union();
        }
    }

    /// Starts scanning for one tag's matches. Returns whether it started.
    fn start_tag_index(&mut self, id: Uuid) -> bool {
        let Some(file) = self.file.clone() else {
            return false;
        };
        let Some(matcher) = self
            .tag_store
            .tags
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.matcher.clone())
        else {
            return false;
        };
        if !matcher.is_valid() || matcher.is_empty() {
            return false;
        }
        self.stop_tag_filter(id);

        let index = Arc::new(MatchIndex::new(matcher, file.length()));
        self.tag_indexes.insert(id, index.clone());
        let ct = CancelToken::new();
        self.tag_cts.insert(id, ct.clone());
        std::thread::spawn(move || index.build(&file, &ct));
        true
    }

    fn stop_tag_filter(&mut self, id: Uuid) {
        if let Some(ct) = self.tag_cts.remove(&id) {
            ct.cancel();
        }
        self.tag_indexes.remove(&id);
    }

    fn rebuild_union(&mut self) {
        if let Some(ct) = self.union_ct.take() {
            ct.cancel();
        }

        let file = self.file.clone();
        if file.is_none() || self.tag_indexes.is_empty() {
            self.tag_union = None;
            self.recompute_active_filter();
            return;
        }
        let file = file.unwrap();

        let sources: Vec<Arc<MatchIndex>> = self.tag_indexes.values().cloned().collect();
        let union = Arc::new(UnionFilterIndex::new(sources, file.length()));
        self.tag_union = Some(union.clone());
        self.last_union_complete = false;

        let ct = CancelToken::new();
        self.union_ct = Some(ct.clone());
        std::thread::spawn(move || union.build(&ct));
        self.recompute_active_filter();
    }

    fn cancel_all_tag_filters(&mut self) {
        if let Some(ct) = self.union_ct.take() {
            ct.cancel();
        }
        self.tag_union = None;
        for (_, ct) in self.tag_cts.drain() {
            ct.cancel();
        }
        self.tag_indexes.clear();
    }

    // -------------------------------------------------------- filter composition

    fn recompute_active_filter(&mut self) {
        let Some(file) = self.file.clone() else {
            self.view.set_active_filter(None);
            return;
        };

        let ad_hoc = self.ad_hoc_filter();
        let ad_hoc_live = ad_hoc.is_some();
        let tag_on = self.tag_union.as_ref().is_some_and(|u| u.is_complete());
        let ad_hoc_complete = self.matches.as_ref().is_some_and(|m| m.is_complete());

        let (mut result, base): (Option<Arc<dyn FilterIndex>>, Base) =
            if ad_hoc_live && tag_on && ad_hoc_complete {
                let ad_hoc = ad_hoc.unwrap();
                let union = self.tag_union.clone().unwrap();
                (
                    Some(Arc::new(IntersectionIndex::new(
                        ad_hoc.as_ref(),
                        union.as_ref(),
                        file.length(),
                    ))),
                    Base::Intersection,
                )
            } else if tag_on {
                // Tag-only, including while the ad-hoc scan is still running.
                (
                    Some(self.tag_union.clone().unwrap() as Arc<dyn FilterIndex>),
                    Base::TagUnion,
                )
            } else if ad_hoc_live {
                // Live, incremental (exclude only appears once the scan is complete).
                (ad_hoc, Base::AdHoc)
            } else {
                (None, Base::None)
            };

        if let Some(zoom) = self.zoom.clone() {
            let zoomed: Arc<dyn FilterIndex> = match &self.slot_filter {
                Some(slots) if slots.is_complete() => slots.clone(),
                _ => zoom,
            };

            // IntersectionIndex needs both sides finished; until then the zoom alone is
            // correct-but-wider, which beats showing nothing.
            let base_ready = match base {
                Base::Intersection => true,
                Base::AdHoc => ad_hoc_complete,
                Base::TagUnion => tag_on,
                Base::None => false,
            };

            result = if base_ready {
                let other = result.unwrap();
                Some(Arc::new(IntersectionIndex::new(
                    zoomed.as_ref(),
                    other.as_ref(),
                    file.length(),
                )))
            } else {
                Some(zoomed)
            };
        }

        if let Some(fit) = self.column_fit.clone().filter(|f| f.is_complete()) {
            let fit: Arc<dyn FilterIndex> = fit;
            result = Some(match result {
                Some(other) => Arc::new(IntersectionIndex::new(
                    fit.as_ref(),
                    other.as_ref(),
                    file.length(),
                )),
                None => fit,
            });
        }

        if self.changes_filter {
            let changes: Arc<dyn FilterIndex> = self.changes.clone();
            result = Some(match result {
                Some(other) => Arc::new(IntersectionIndex::new(
                    changes.as_ref(),
                    other.as_ref(),
                    file.length(),
                )),
                None => changes,
            });
        }

        self.view.set_active_filter(result);
    }

    /// What the search box contributes to the filter: its matches, or with Exclude on, every line
    /// that isn't one. The complement waits for the scan to finish — until then lines that haven't
    /// been tested yet would wrongly pass — so meanwhile the view stays unfiltered.
    fn ad_hoc_filter(&self) -> Option<Arc<dyn FilterIndex>> {
        if !self.filter_on {
            return None;
        }
        let matches = self.matches.clone()?;
        if !self.exclude_on {
            return Some(matches);
        }
        if !matches.is_complete() {
            return None;
        }
        let lines = self.file.as_ref()?.line_count();
        Some(Arc::new(ComplementIndex::new(matches, lines)))
    }

    // ------------------------------------------------------------ semantic zoom

    /// Ctrl+M steps out one zoom level: lines -> patterns, zoomed-in -> patterns, patterns -> lines.
    fn toggle_semantic_zoom(&mut self) {
        if self.file.is_none() {
            return;
        }
        if self.view.mode() == LogViewMode::Templates {
            self.leave_templates();
            return;
        }
        if self.zoom.is_some() {
            self.clear_zoom();
            self.show_template_list();
            return;
        }
        self.start_clustering();
    }

    fn start_clustering(&mut self) {
        self.cancel_clustering();
        let Some(file) = self.file.clone() else {
            return;
        };

        // Cluster what the user is currently looking at, not always the whole file.
        let scope = self.current_scope_filter();
        self.templates_whole_file = scope.is_none();

        let index = Arc::new(TemplateIndex::new(file.length() as i64));
        self.templates = Some(index.clone());

        let ct = CancelToken::new();
        self.template_ct = Some(ct.clone());
        std::thread::spawn(move || {
            index.build(&file, scope.as_ref().map(|s| &**s as &dyn FilterIndex), &ct);
        });

        self.search.clear();
        self.show_template_list();
    }

    /// The filter the template pass should honour: whatever is driving the view right now. Both
    /// sides must be complete before they can be composed, so a scan still in flight simply means
    /// we cluster the wider set.
    fn current_scope_filter(&self) -> Option<Arc<dyn FilterIndex>> {
        let file = self.file.as_ref()?;

        let ad_hoc = self
            .ad_hoc_filter()
            .filter(|_| self.matches.as_ref().is_some_and(|m| m.is_complete()));
        let tag_on = self.tag_union.as_ref().is_some_and(|u| u.is_complete());

        if let (Some(ad_hoc), true) = (&ad_hoc, tag_on) {
            let union = self.tag_union.clone().unwrap();
            return Some(Arc::new(IntersectionIndex::new(
                ad_hoc.as_ref(),
                union.as_ref(),
                file.length(),
            )));
        }
        if tag_on {
            return Some(self.tag_union.clone().unwrap());
        }
        ad_hoc
    }

    fn show_template_list(&mut self) {
        let Some(index) = self.templates.clone() else {
            return;
        };
        let all = index.snapshot_sorted();
        self.view
            .show_templates(self.filter_template_list(all), index.rare_threshold());
    }

    /// In Templates mode the search box is a plain substring filter over the pattern list — no
    /// worker, no index, instant.
    fn filter_template_list(&self, all: Vec<Arc<LogTemplate>>) -> Vec<Arc<LogTemplate>> {
        let needle = self.search.trim().to_lowercase();
        if needle.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|t| t.render().to_lowercase().contains(&needle))
            .collect()
    }

    fn leave_templates(&mut self) {
        self.view.show_lines();
        self.search.clear();
        self.start_match_scan();
    }

    fn zoom_into_selected_template(&mut self) {
        let Some(template) = self.view.selected_template() else {
            return;
        };
        self.zoom = Some(template);
        self.search.clear();
        self.view.show_lines();
        self.recompute_active_filter();
    }

    fn clear_zoom(&mut self) {
        self.cancel_slot_filter();
        self.zoom = None;
        self.slot_error = None;
        self.search.clear();
        self.count_error = false;
        self.count_text.clear();
    }

    /// Escape with an empty search box means "undo the narrowest thing I did".
    fn step_out_one_zoom_level(&mut self) {
        if self.view.mode() == LogViewMode::Templates {
            self.leave_templates();
            return;
        }
        if self.zoom.is_some() {
            self.clear_zoom();
            self.show_template_list();
        }
    }

    fn start_slot_filter(&mut self) {
        self.cancel_slot_filter();
        let (Some(file), Some(zoom)) = (self.file.clone(), self.zoom.clone()) else {
            return;
        };

        let query = SlotQuery::parse(&self.search, zoom.slot_count());
        if let Some(error) = query.error.clone() {
            self.slot_error = Some(error);
            self.count_error = true;
            self.count_text = "invalid slot filter".to_string();
            self.recompute_active_filter();
            return;
        }

        self.slot_error = None;
        self.count_error = false;

        let prefix = self
            .templates
            .as_ref()
            .map(|t| t.prefix())
            .unwrap_or(fastlogviewer_core::PrefixShape::NONE);

        let index = Arc::new(SlotFilterIndex::new(
            zoom,
            Arc::new(query),
            prefix,
            file.length(),
        ));
        self.slot_filter = Some(index.clone());

        let ct = CancelToken::new();
        self.slot_ct = Some(ct.clone());
        std::thread::spawn(move || index.build(&file, &ct));
        self.recompute_active_filter();
    }

    fn cancel_clustering(&mut self) {
        if let Some(ct) = self.template_ct.take() {
            ct.cancel();
        }
        self.templates = None;
    }

    fn cancel_slot_filter(&mut self) {
        if let Some(ct) = self.slot_ct.take() {
            ct.cancel();
        }
        self.slot_filter = None;
    }

    // ------------------------------------------------------------- navigation

    fn jump_match(&mut self, direction: i32) {
        let Some(matches) = self.matches.clone() else {
            return;
        };
        if matches.count() == 0 {
            return;
        }

        let current = self.view.selected_actual_line();
        let i = if direction > 0 {
            let i = matches.index_at_or_after(current as i32 + 1);
            if i >= matches.count() {
                0 // wrap
            } else {
                i
            }
        } else {
            let from = if current < 0 {
                i32::MAX
            } else {
                current as i32
            };
            let i = matches.index_at_or_after(from);
            if i == 0 {
                matches.count() - 1 // wrap
            } else {
                i - 1
            }
        };

        let line = matches.get(i) as i64;
        self.view.current_match_line = line;
        self.view.go_to_actual(line);
    }

    fn open_dialog(&mut self, folder: Option<&Path>) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        let picked = dialog
            .add_filter("Log files", &["log", "txt"])
            .add_filter("All files", &["*"])
            .pick_file();
        match picked {
            Some(path) if self.file.is_some() => self.open_request = Some(path),
            Some(path) => self.load_file(&path.to_string_lossy()),
            None => {}
        }
    }

    // ------------------------------------------------------------------ stats

    fn stop_stats(&mut self) {
        for (_, scan) in self.stats_scans.drain() {
            scan.ct.cancel();
        }
        for (_, shown) in self.stats_shown.drain() {
            shown.ct.cancel();
        }
        self.stats_prefix = None;
    }

    /// The match index the stats panel reads for a tag: the filter's when the tag filters,
    /// otherwise the panel's own.
    fn stats_index(&self, id: &Uuid) -> Option<Arc<MatchIndex>> {
        self.tag_indexes
            .get(id)
            .or_else(|| self.stats_scans.get(id).map(|scan| &scan.index))
            .cloned()
    }

    /// Keeps one match index per tag while the stats panel is open, and the "of shown" counts
    /// in step with the current filter. Cheap when nothing changed, so it runs after every update.
    fn sync_stats(&mut self) {
        if self.side_panel != Some(SidePanel::Stats) {
            if !self.stats_scans.is_empty() || !self.stats_shown.is_empty() {
                self.stop_stats();
            }
            return;
        }
        let Some(file) = self.file.clone() else {
            return;
        };

        let mut wanted = Vec::new();
        for tag in &self.tag_store.tags {
            let Some(matcher) = tag.matcher.clone() else {
                continue;
            };
            if matcher.is_valid() && !matcher.is_empty() && !self.tag_indexes.contains_key(&tag.id)
            {
                wanted.push((tag.id, matcher));
            }
        }
        let stale: Vec<Uuid> = self
            .stats_scans
            .iter()
            .filter(|(id, scan)| {
                !wanted
                    .iter()
                    .any(|(want, matcher)| want == *id && Arc::ptr_eq(matcher, &scan.matcher))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(scan) = self.stats_scans.remove(&id) {
                scan.ct.cancel();
            }
        }
        for (id, matcher) in wanted {
            if self.stats_scans.contains_key(&id) {
                continue;
            }
            let index = Arc::new(MatchIndex::new(matcher.clone(), file.length()));
            let ct = CancelToken::new();
            let (building, worker_ct, worker_file) = (index.clone(), ct.clone(), file.clone());
            std::thread::spawn(move || building.build(&worker_file, &worker_ct));
            self.stats_scans
                .insert(id, StatsScan { matcher, index, ct });
        }

        if self.stats_prefix.is_none() && file.index_complete() {
            let sample: Vec<Vec<u8>> = (0..file.line_count().min(200))
                .map(|line| file.get_line(line, 512).into_bytes())
                .collect();
            self.stats_prefix = Some(PrefixShape::sniff(&sample));
        }

        self.sync_shown_counts();
    }

    /// Recounts each tag's matches among the displayed lines whenever the filter or the tag's
    /// index changes. Counting waits for the tag's scan to finish.
    fn sync_shown_counts(&mut self) {
        let Some(filter) = self.view.active_filter() else {
            for (_, shown) in self.stats_shown.drain() {
                shown.ct.cancel();
            }
            return;
        };
        let ids: Vec<Uuid> = self.tag_store.tags.iter().map(|t| t.id).collect();
        self.stats_shown.retain(|id, shown| {
            let keep = ids.contains(id);
            if !keep {
                shown.ct.cancel();
            }
            keep
        });

        for id in ids {
            let Some(index) = self.stats_index(&id).filter(|i| i.is_complete()) else {
                continue;
            };
            let key = [
                Arc::as_ptr(&filter) as *const () as usize,
                filter.count(),
                Arc::as_ptr(&index) as usize,
                index.count(),
            ];
            if self.stats_shown.get(&id).is_some_and(|s| s.key == key) {
                continue;
            }
            if let Some(old) = self.stats_shown.remove(&id) {
                old.ct.cancel();
            }
            let count = Arc::new(AtomicI64::new(-1));
            let ct = CancelToken::new();
            let (result, worker_ct, worker_filter) = (count.clone(), ct.clone(), filter.clone());
            std::thread::spawn(move || {
                if let Some(n) =
                    intersection_count(index.as_ref(), worker_filter.as_ref(), &worker_ct)
                {
                    result.store(n as i64, Ordering::Release);
                }
            });
            self.stats_shown.insert(id, ShownCount { key, count, ct });
        }
    }

    fn stats_busy(&self) -> bool {
        self.side_panel == Some(SidePanel::Stats)
            && (self.stats_scans.values().any(|s| !s.index.is_complete())
                || self
                    .stats_shown
                    .values()
                    .any(|s| s.count.load(Ordering::Acquire) < 0))
    }

    /// The leading timestamp of a line, or "" when it has none the sniffed shape recognises.
    fn timestamp_of(&self, file: &LogFile, line: usize) -> String {
        let Some(shape) = self.stats_prefix else {
            return String::new();
        };
        let mut buf = [0u8; 256];
        let len = file.copy_line(line, &mut buf);
        let ts = shape.measure_timestamp(&buf, len);
        String::from_utf8_lossy(&buf[..ts.min(len)])
            .trim()
            .to_string()
    }

    fn tag_stats(&self) -> Vec<TagStat> {
        let Some(file) = self.file.as_ref() else {
            return Vec::new();
        };
        let total = file.line_count();
        let shown_total = self.view.display_count();
        let mut stats = Vec::new();
        for tag in &self.tag_store.tags {
            let Some(index) = self.stats_index(&tag.id) else {
                continue;
            };
            let count = index.count();
            let end = |i: usize| {
                let line = index.get(i) as i64;
                (line, self.timestamp_of(file, line as usize))
            };
            let [r, g, b] = tag.color();
            stats.push(TagStat {
                id: tag.id,
                label: tag.label.clone(),
                color: Color::from_rgb8(r, g, b),
                count,
                complete: index.is_complete(),
                share: percent(count, total),
                shown_share: self
                    .stats_shown
                    .get(&tag.id)
                    .map(|s| s.count.load(Ordering::Acquire))
                    .filter(|n| *n >= 0)
                    .map(|n| percent(n as usize, shown_total)),
                buckets: bucket_counts(index.as_ref(), total, STRIP_BUCKETS),
                first: (count > 0).then(|| end(0)),
                last: (count > 0 && index.is_complete()).then(|| end(count - 1)),
            });
        }
        stats
    }

    fn apply_stats_message(&mut self, message: StatsMessage) {
        match message {
            StatsMessage::Close => self.side_panel = None,
            StatsMessage::JumpToLine(line) => self.go_to_line_in_lines(line),
            StatsMessage::JumpToBucket(id, bucket) => {
                let (Some(file), Some(index)) = (self.file.clone(), self.stats_index(&id)) else {
                    return;
                };
                let total = file.line_count();
                let start = bucket_start(bucket, total, STRIP_BUCKETS);
                let end = bucket_start(bucket + 1, total, STRIP_BUCKETS);
                let i = index.index_at_or_after(start as i32);
                if i < index.count() && (index.get(i) as usize) < end.max(start + 1) {
                    self.go_to_line_in_lines(index.get(i) as i64);
                }
            }
        }
    }

    /// Jumps to a real file line, leaving the pattern list first if it is showing.
    fn go_to_line_in_lines(&mut self, line: i64) {
        if self.view.mode() == LogViewMode::Templates {
            self.leave_templates();
        }
        self.view.go_to_actual(line);
    }

    // ------------------------------------------------------------------ notes

    /// Reads the file's notes and starts matching the line notes to its text, which has to wait
    /// for the line index.
    fn load_notes(&mut self, file: &Arc<LogFile>) {
        let path = file.path();
        self.notes_read_only = None;
        self.notes = match load_notes(path) {
            LoadedNotes::Notes(notes) => notes,
            LoadedNotes::Foreign => {
                self.notes_read_only = Some(format!(
                    "{} belongs to another log or can't be read, so notes won't be saved.",
                    notes_path(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                NotesFile::for_log(path)
            }
        };
        self.pattern_matches = vec![None; self.notes.pattern_notes.len()];
        self.pattern_matches_from = 0;

        if self.notes.line_notes.is_empty() {
            self.notes_placed = true;
            self.refresh_note_marks();
            return;
        }
        let result = Arc::new(Mutex::new(None));
        let ct = CancelToken::new();
        let (worker_result, worker_ct, worker_file) = (result.clone(), ct.clone(), file.clone());
        let notes = self.notes.line_notes.clone();
        std::thread::spawn(move || {
            while !worker_file.index_complete() {
                if worker_ct.is_cancelled() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some(pins) = pin_line_notes(&worker_file, &notes, &worker_ct) {
                *worker_result.lock().unwrap() = Some(pins);
            }
        });
        self.notes_pin = Some(NotesPin { result, ct });
    }

    fn release_notes(&mut self) {
        if let Some(pin) = self.notes_pin.take() {
            pin.ct.cancel();
        }
        self.notes = NotesFile::for_log(Path::new(""));
        self.notes_read_only = None;
        self.notes_placed = false;
        self.note_focus = None;
        self.pattern_matches.clear();
        self.pattern_matches_from = 0;
        self.view.set_note_marks(HashSet::new(), Vec::new());
    }

    /// Picks up the finished line-note matching and re-matches pattern notes when a clustering
    /// finishes. Runs after every update.
    fn sync_notes(&mut self) {
        let pins = self
            .notes_pin
            .as_ref()
            .and_then(|pin| pin.result.lock().unwrap().take());
        if let Some(pins) = pins {
            self.notes_pin = None;
            let before = self.notes.line_notes.len();
            let mut moved = false;
            let mut kept = Vec::with_capacity(before);
            for (mut note, pin) in self.notes.line_notes.drain(..).zip(pins) {
                if let Some(line) = pin {
                    moved |= note.line != line;
                    note.line = line;
                    kept.push(note);
                }
            }
            self.notes.line_notes = kept;
            self.notes_placed = true;
            if moved || self.notes.line_notes.len() != before {
                self.save_notes();
            }
            self.refresh_note_marks();
        }

        let finished = self.templates.clone().filter(|t| t.is_complete());
        let from = finished.as_ref().map_or(0, |t| Arc::as_ptr(t) as usize);
        if from != self.pattern_matches_from {
            self.pattern_matches_from = from;
            self.match_pattern_notes();
        }
    }

    /// Finds each pattern note's pattern in the last finished clustering. After a clustering of
    /// the whole file, a pattern note that matched nothing is dropped; after a partial one it is
    /// kept and listed as not matched.
    fn match_pattern_notes(&mut self) {
        let Some(index) = self.templates.clone().filter(|t| t.is_complete()) else {
            self.pattern_matches = vec![None; self.notes.pattern_notes.len()];
            self.refresh_note_marks();
            return;
        };
        let all: Vec<(String, Arc<LogTemplate>)> = index
            .snapshot_sorted()
            .into_iter()
            .map(|t| (t.render(), t))
            .collect();
        let find = |pattern: &str| {
            all.iter()
                .find(|(text, _)| text == pattern)
                .map(|(_, t)| t.clone())
        };
        self.pattern_matches = self
            .notes
            .pattern_notes
            .iter()
            .map(|n| find(&n.pattern))
            .collect();

        if self.templates_whole_file && self.pattern_matches.iter().any(|m| m.is_none()) {
            let matches = std::mem::take(&mut self.pattern_matches);
            let mut notes = Vec::new();
            for (note, found) in self.notes.pattern_notes.drain(..).zip(matches) {
                if let Some(template) = found {
                    notes.push(note);
                    self.pattern_matches.push(Some(template));
                }
            }
            self.notes.pattern_notes = notes;
            self.save_notes();
        }
        self.refresh_note_marks();
    }

    fn save_notes(&mut self) {
        if self.notes_read_only.is_some() {
            return;
        }
        let Some(file) = self.file.clone() else {
            return;
        };
        if let Err(e) = save_notes(file.path(), &self.notes) {
            self.notify(format!("Couldn't save notes: {e}"), true);
        }
    }

    fn refresh_note_marks(&mut self) {
        let lines = if self.notes_placed {
            self.notes
                .line_notes
                .iter()
                .map(|n| n.line as i64)
                .collect()
        } else {
            HashSet::new()
        };
        let patterns = self.pattern_matches.iter().flatten().cloned().collect();
        self.view.set_note_marks(lines, patterns);
    }

    /// The pattern a real line belongs to in the last finished clustering.
    fn template_of_line(&self, line: i64) -> Option<Arc<LogTemplate>> {
        let index = self.templates.as_ref().filter(|t| t.is_complete())?;
        index.snapshot_sorted().into_iter().find(|t| {
            let i = t.index_at_or_after(line as i32);
            i < t.count() && t.get(i) as i64 == line
        })
    }

    /// Opens the notes panel on a line: its note, and its pattern's note when patterns exist.
    fn focus_note_line(&mut self, line: i64) {
        let note = self
            .notes
            .line_notes
            .iter()
            .find(|n| n.line as i64 == line)
            .map(|n| n.note.clone())
            .unwrap_or_default();
        self.line_editor = text_editor::Content::with_text(&note);
        let pattern = self.template_of_line(line).map(|t| t.render());
        self.set_pattern_editor(pattern.as_deref());
        self.note_focus = Some(NoteFocus {
            line: Some(line),
            pattern,
        });
        self.side_panel = Some(SidePanel::Notes);
    }

    fn focus_note_pattern(&mut self, pattern: String) {
        self.set_pattern_editor(Some(&pattern));
        self.note_focus = Some(NoteFocus {
            line: None,
            pattern: Some(pattern),
        });
        self.side_panel = Some(SidePanel::Notes);
    }

    fn set_pattern_editor(&mut self, pattern: Option<&str>) {
        let note = pattern
            .and_then(|p| self.notes.pattern_notes.iter().find(|n| n.pattern == p))
            .map(|n| n.note.clone())
            .unwrap_or_default();
        self.pattern_editor = text_editor::Content::with_text(&note);
    }

    /// Ctrl+Shift+N: a note for the selected line, or in the pattern list for the selected
    /// pattern.
    fn add_note_for_selection(&mut self) {
        if self.view.mode() == LogViewMode::Templates {
            let Some(template) = self.view.selected_template() else {
                return;
            };
            if !self.templates.as_ref().is_some_and(|t| t.is_complete()) {
                self.notify(
                    "Patterns are still being found; add the note once they finish",
                    true,
                );
                return;
            }
            self.focus_note_pattern(template.render());
            return;
        }
        let line = self.view.selected_actual_line();
        if line >= 0 {
            self.focus_note_line(line);
        }
    }

    /// The text of a note editor, without the trailing newline the editor keeps.
    fn editor_text(content: &text_editor::Content) -> String {
        content.text().trim_end_matches(['\r', '\n']).to_string()
    }

    fn edit_line_note(&mut self, action: text_editor::Action) {
        let is_edit = action.is_edit();
        self.line_editor.perform(action);
        if !is_edit || !self.notes_placed {
            return;
        }
        let (Some(file), Some(line)) = (
            self.file.clone(),
            self.note_focus.as_ref().and_then(|f| f.line),
        ) else {
            return;
        };
        let text = Self::editor_text(&self.line_editor);
        let existing = self
            .notes
            .line_notes
            .iter()
            .position(|n| n.line as i64 == line);
        match (existing, text.trim().is_empty()) {
            (Some(i), true) => {
                self.notes.line_notes.remove(i);
            }
            (Some(i), false) => {
                let note = &mut self.notes.line_notes[i];
                note.note = text;
                note.updated = unix_now();
            }
            (None, true) => return,
            (None, false) => self.notes.line_notes.push(LineNote {
                text: copy_text_of(&file, line as usize),
                line: line as usize,
                note: text,
                created: unix_now(),
                updated: unix_now(),
            }),
        }
        self.save_notes();
        self.refresh_note_marks();
    }

    fn edit_pattern_note(&mut self, action: text_editor::Action) {
        let is_edit = action.is_edit();
        self.pattern_editor.perform(action);
        let Some(pattern) = self.note_focus.as_ref().and_then(|f| f.pattern.clone()) else {
            return;
        };
        if !is_edit {
            return;
        }
        let text = Self::editor_text(&self.pattern_editor);
        let existing = self
            .notes
            .pattern_notes
            .iter()
            .position(|n| n.pattern == pattern);
        match (existing, text.trim().is_empty()) {
            (Some(i), true) => {
                self.notes.pattern_notes.remove(i);
                self.pattern_matches.remove(i);
            }
            (Some(i), false) => {
                let note = &mut self.notes.pattern_notes[i];
                note.note = text;
                note.updated = unix_now();
            }
            (None, true) => return,
            (None, false) => {
                self.notes.pattern_notes.push(PatternNote {
                    pattern: pattern.clone(),
                    note: text,
                    created: unix_now(),
                    updated: unix_now(),
                });
                let found = self
                    .templates
                    .as_ref()
                    .filter(|t| t.is_complete())
                    .and_then(|t| {
                        t.snapshot_sorted()
                            .into_iter()
                            .find(|t| t.render() == pattern)
                    });
                self.pattern_matches.push(found);
            }
        }
        self.save_notes();
        self.refresh_note_marks();
    }

    fn apply_notes_message(&mut self, message: NotesMessage) {
        match message {
            NotesMessage::Close => self.side_panel = None,
            NotesMessage::EditLine(action) => self.edit_line_note(action),
            NotesMessage::EditPattern(action) => self.edit_pattern_note(action),
            NotesMessage::JumpToLine(line) => {
                self.go_to_line_in_lines(line);
                self.focus_note_line(line);
            }
            NotesMessage::FocusPattern(i) => {
                if let Some(note) = self.notes.pattern_notes.get(i) {
                    self.focus_note_pattern(note.pattern.clone());
                }
            }
            NotesMessage::DeleteLine(i) => {
                let mut order: Vec<usize> = (0..self.notes.line_notes.len()).collect();
                order.sort_by_key(|&n| self.notes.line_notes[n].line);
                let Some(&index) = order.get(i) else {
                    return;
                };
                let removed = self.notes.line_notes.remove(index);
                if self.note_focus.as_ref().and_then(|f| f.line) == Some(removed.line as i64) {
                    self.line_editor = text_editor::Content::new();
                }
                self.save_notes();
                self.refresh_note_marks();
            }
            NotesMessage::DeletePattern(i) => {
                if i >= self.notes.pattern_notes.len() {
                    return;
                }
                let removed = self.notes.pattern_notes.remove(i);
                self.pattern_matches.remove(i);
                if self.note_focus.as_ref().and_then(|f| f.pattern.as_deref())
                    == Some(removed.pattern.as_str())
                {
                    self.pattern_editor = text_editor::Content::new();
                }
                self.save_notes();
                self.refresh_note_marks();
            }
        }
    }

    fn notes_view(&self) -> Element<'_, Message> {
        let focus = self.note_focus.as_ref().map(|focus| notes_ui::Focus {
            line: focus.line.and_then(|line| {
                let file = self.file.as_ref()?;
                Some((line, file.get_line(line as usize, 512)))
            }),
            line_editor: &self.line_editor,
            pattern: focus.pattern.clone(),
            pattern_editor: &self.pattern_editor,
        });

        let mut lines: Vec<LineRow> = self
            .notes
            .line_notes
            .iter()
            .map(|n| LineRow {
                line: n.line,
                note: n.note.clone(),
            })
            .collect();
        // Same order `DeleteLine` indexes by.
        lines.sort_by_key(|row| row.line);

        let clustered = self.templates.as_ref().is_some_and(|t| t.is_complete());
        let patterns = self
            .notes
            .pattern_notes
            .iter()
            .zip(&self.pattern_matches)
            .map(|(n, found)| PatternRow {
                pattern: n.pattern.clone(),
                note: n.note.clone(),
                status: match (found, clustered) {
                    (Some(_), _) => None,
                    (None, true) => Some("not matched in this run"),
                    (None, false) => Some("run Patterns to show"),
                },
            })
            .collect();

        notes_panel(notes_ui::Panel {
            focus,
            lines: if self.notes_placed { lines } else { Vec::new() },
            patterns,
            placing: !self.notes_placed,
            read_only: self.notes_read_only.clone(),
        })
        .map(Message::Notes)
        .boxed()
    }

    // ---------------------------------------------------------------- columns

    /// Lines sampled to find the column layout, spread evenly over the file.
    const COLUMN_SAMPLE: usize = 2000;

    /// Finds the column layout for a newly indexed file while Columns is on, and starts finding
    /// the lines that fit it when the rest are to be hidden. Turns Columns back off, with a
    /// notice, when the file has no layout.
    fn sync_columns(&mut self, file: &Arc<LogFile>) {
        let file_key = Arc::as_ptr(file) as usize;
        if !self.columns_wanted || self.columns_for == file_key {
            return;
        }
        self.columns_for = file_key;
        let lines = file.line_count();
        let step = lines.div_ceil(Self::COLUMN_SAMPLE).max(1);
        let sample: Vec<String> = (0..lines)
            .step_by(step)
            .map(|line| copy_text_of(file, line))
            .collect();
        self.cancel_column_fit();
        let delimiter = self.column_settings.delimiter.clone();
        match detect_columns(&sample, delimiter.as_deref()) {
            Some(shape) => {
                let shape = Arc::new(shape);
                self.view.set_columns(Some(shape.clone()));
                if self.column_settings.hide_misfits {
                    self.start_column_fit(file, shape);
                }
            }
            None => {
                self.columns_wanted = false;
                self.view.set_columns(None);
                let notice = match delimiter.as_deref() {
                    Some(" ") => "No columns split by spaces in this file".to_string(),
                    Some(d) => format!("No columns split by \"{d}\" in this file"),
                    None => "No column layout found in this file".to_string(),
                };
                self.notify(notice, true);
            }
        }
        self.recompute_active_filter();
    }

    fn start_column_fit(&mut self, file: &Arc<LogFile>, shape: Arc<ColumnShape>) {
        let index = Arc::new(ColumnFitIndex::new(shape, file.length()));
        self.column_fit = Some(index.clone());
        let ct = CancelToken::new();
        self.column_fit_ct = Some(ct.clone());
        let file = file.clone();
        std::thread::spawn(move || index.build(&file, &ct));
    }

    fn cancel_column_fit(&mut self) {
        if let Some(ct) = self.column_fit_ct.take() {
            ct.cancel();
        }
        self.column_fit = None;
        // So the next fit finishing is seen as a change even if the last one had finished too.
        self.last_fit_complete = false;
    }

    fn columns_off(&mut self) {
        self.columns_wanted = false;
        self.columns_for = 0;
        self.view.set_columns(None);
        self.cancel_column_fit();
        self.recompute_active_filter();
    }

    fn apply_columns_message(&mut self, message: ColumnsMessage) -> Task<Message> {
        let Some(dialog) = self.columns_dialog.as_mut() else {
            return Task::none();
        };
        match message {
            ColumnsMessage::Auto => dialog.auto = true,
            ColumnsMessage::Custom => {
                dialog.auto = false;
                return operation::focus(tag_ui::COLUMNS_INPUT);
            }
            ColumnsMessage::Delimiter(delimiter) => {
                dialog.auto = false;
                dialog.delimiter = delimiter;
            }
            ColumnsMessage::HideMisfits(on) => dialog.hide_misfits = on,
            ColumnsMessage::Apply => {
                let Some(settings) = dialog.settings() else {
                    return Task::none();
                };
                self.columns_dialog = None;
                settings.save();
                self.column_settings = settings;
                self.columns_wanted = true;
                self.columns_for = 0;
                if let Some(file) = self.file.clone().filter(|f| f.index_complete()) {
                    self.sync_columns(&file);
                }
            }
            ColumnsMessage::Cancel => self.columns_dialog = None,
        }
        Task::none()
    }

    // ------------------------------------------------------------------- copy

    fn copy(&mut self, kind: CopyKind) -> Task<Message> {
        let Some(file) = self.file.clone() else {
            return Task::none();
        };
        let joined = match kind {
            CopyKind::Selection => match self.view.selected_text(COPY_LIMIT_BYTES) {
                Some(joined) => joined,
                None => return self.copy(CopyKind::Lines),
            },
            CopyKind::Line => {
                let line = self.view.selected_actual_line();
                if line < 0 || self.view.mode() != LogViewMode::Lines {
                    return Task::none();
                }
                join_lines(&file, std::iter::once(line as usize), COPY_LIMIT_BYTES)
            }
            CopyKind::Lines => {
                let Some((first, last)) = self.view.selected_line_range() else {
                    return Task::none();
                };
                let lines = (first..=last).map(|display| self.view.line_at(display) as usize);
                join_lines(&file, lines, COPY_LIMIT_BYTES)
            }
            CopyKind::Range(first, last) => join_lines(&file, first..=last, COPY_LIMIT_BYTES),
        };
        self.report_copy(&joined);
        // An empty clipboard write is ignored, so an empty line still copies as something.
        iced::clipboard::write(if joined.text.is_empty() {
            " ".to_string()
        } else {
            joined.text
        })
        .discard()
    }

    fn report_copy(&mut self, joined: &Joined) {
        let lines = thousands(joined.lines as u64);
        if joined.truncated {
            self.notify(
                format!(
                    "Copied only the first {lines} lines: a copy stops at {} MB",
                    COPY_LIMIT_BYTES >> 20
                ),
                true,
            );
        } else if joined.lines == 1 {
            self.notify("Copied", false);
        } else {
            self.notify(format!("Copied {lines} lines"), false);
        }
    }

    fn open_copy_dialog(&mut self) -> Task<Message> {
        if self.file.is_none() {
            return Task::none();
        }
        self.copy_open = true;
        self.copy_error = None;
        operation::focus(tag_ui::COPY_RANGE_INPUT)
    }

    fn close_copy(&mut self) {
        self.copy_open = false;
        self.copy_error = None;
    }

    fn apply_copy_message(&mut self, message: CopyMessage) -> Task<Message> {
        let kind = match message {
            CopyMessage::Selection => CopyKind::Selection,
            CopyMessage::Line => CopyKind::Line,
            CopyMessage::Lines => CopyKind::Lines,
            CopyMessage::RangeInput(text) => {
                self.copy_range = text;
                self.copy_error = None;
                return Task::none();
            }
            CopyMessage::Range => {
                let lines = self.file.as_ref().map_or(0, |f| f.line_count());
                match parse_line_range(&self.copy_range, lines) {
                    Ok((first, last)) => CopyKind::Range(first, last),
                    Err(error) => {
                        self.copy_error = Some(error);
                        return Task::none();
                    }
                }
            }
            CopyMessage::Cancel => {
                self.close_copy();
                return Task::none();
            }
        };
        self.close_copy();
        self.copy(kind)
    }

    // ----------------------------------------------------------------- status

    /// "  ·  line N of M (P%)" for the selected row, or the top row when nothing is selected.
    /// Filtered, N and M count displayed rows and the real line number follows.
    fn position_text(&self) -> String {
        let Some(display) = self.view.current_display() else {
            return String::new();
        };
        let count = self.view.display_count();
        let pct = 100.0 * (display + 1) as f64 / count as f64;
        if self.view.filter_on() {
            format!(
                "  ·  row {} of {} ({:.0}%)  ·  line {}",
                thousands(display as u64 + 1),
                thousands(count as u64),
                pct,
                thousands(self.view.line_at(display) as u64 + 1)
            )
        } else {
            format!(
                "  ·  line {} of {} ({:.0}%)",
                thousands(display as u64 + 1),
                thousands(count as u64),
                pct
            )
        }
    }

    fn refresh_status(&mut self) {
        let Some(file) = self.file.clone() else {
            self.busy = false;
            return;
        };

        let size = format!("{} MB", thousands_1dp(file.length() as f64 / 1_048_576.0));
        let mut busy = false;
        let mut left;

        if file.index_complete() {
            self.sync_columns(&file);
            self.apply_pending_restore(&file);
            left = format!(
                "{} lines  ·  {}{}{}",
                thousands(file.line_count() as u64),
                size,
                self.position_text(),
                if self.view.filter_on() {
                    "  ·  filtered"
                } else {
                    ""
                }
            );
        } else {
            busy = true;
            let pct = if file.length() == 0 {
                100.0
            } else {
                100.0 * file.scanned() as f64 / file.length() as f64
            };
            left = format!(
                "{} lines  ·  {}  ·  indexing {:.0}%",
                thousands(file.line_count() as u64),
                size,
                pct
            );
        }

        // Semantic zoom owns the status line whenever it is active.
        let mut publish_templates = false;
        let clustering = self.templates.as_ref().is_some_and(|t| !t.is_complete());
        if clustering {
            let t = self.templates.clone().unwrap();
            busy = true;
            let pct = if t.scope_count() == 0 {
                0.0
            } else {
                100.0 * t.progress() as f64 / t.scope_count() as f64
            };
            left = format!(
                "clustering… {:.0}%  ·  {} patterns so far",
                pct,
                thousands(t.template_count() as u64)
            );
            if self.view.mode() == LogViewMode::Templates {
                publish_templates = true;
            }
        } else if self.view.mode() == LogViewMode::Templates && self.templates.is_some() {
            publish_templates = true; // final publish once the build settles
            let t = self.templates.clone().unwrap();
            left = format!(
                "{} templates  ·  {} lines",
                thousands(t.template_count() as u64),
                thousands(t.clustered_lines() as u64)
            );
        } else if let Some(zoom) = self.zoom.clone() {
            left = format!(
                "▲ {}  ·  {} lines",
                zoom.render(),
                thousands(zoom.count() as u64)
            );
        }
        if publish_templates {
            self.show_template_list();
        }
        self.status_left = left;

        if let (Some(slots), Some(zoom)) = (self.slot_filter.clone(), self.zoom.clone()) {
            self.count_text = if slots.is_complete() {
                format!(
                    "{} of {}",
                    thousands(slots.count() as u64),
                    thousands(zoom.count() as u64)
                )
            } else {
                format!(
                    "{} of {} · scanning",
                    thousands(slots.count() as u64),
                    thousands(zoom.count() as u64)
                )
            };
            if !slots.is_complete() {
                busy = true;
            }
        } else if self.slot_error.is_some() {
            self.count_error = true;
            self.count_text = "invalid slot filter".to_string();
        } else if let Some(matches) = self.matches.clone() {
            self.count_text = if matches.is_complete() && self.filter_on && self.exclude_on {
                format!("{} lines hidden", thousands(matches.count() as u64))
            } else if matches.is_complete() {
                format!("{} matches", thousands(matches.count() as u64))
            } else {
                format!("{} matches · scanning", thousands(matches.count() as u64))
            };
            if !matches.is_complete() {
                busy = true;
            }
        }

        if self.tag_union.as_ref().is_some_and(|u| !u.is_complete()) {
            busy = true;
        }
        if self.column_fit.as_ref().is_some_and(|f| !f.is_complete()) {
            busy = true;
        }
        if self.tag_indexes.values().any(|i| !i.is_complete()) {
            busy = true;
        }

        let ad_hoc_complete = self.matches.as_ref().is_some_and(|m| m.is_complete());
        let union_complete = self.tag_union.as_ref().is_some_and(|u| u.is_complete());
        let slot_complete = self.slot_filter.as_ref().is_some_and(|s| s.is_complete());
        let fit_complete = self.column_fit.as_ref().is_some_and(|f| f.is_complete());
        if ad_hoc_complete != self.last_adhoc_complete
            || union_complete != self.last_union_complete
            || slot_complete != self.last_slot_complete
            || fit_complete != self.last_fit_complete
        {
            self.last_adhoc_complete = ad_hoc_complete;
            self.last_union_complete = union_complete;
            self.last_slot_complete = slot_complete;
            self.last_fit_complete = fit_complete;
            self.recompute_active_filter();
        }

        // The C# stopped a 100 ms WinForms timer here; `subscription` drops its tick instead.
        self.busy = busy;
    }

    // ------------------------------------------------------------------- tags

    fn apply_chip_action(&mut self, id: Uuid, action: ChipAction) -> Task<Message> {
        let Some(index) = self.tag_store.tags.iter().position(|t| t.id == id) else {
            return Task::none();
        };

        match action {
            ChipAction::ToggleHighlight => {
                self.tag_store.tags[index].highlight_on = !self.tag_store.tags[index].highlight_on;
                self.tag_store.save();
                self.tags_changed = true;
                self.sync_view_tags();
            }
            ChipAction::ToggleFilter => {
                self.tag_store.tags[index].filter_on = !self.tag_store.tags[index].filter_on;
                self.tag_store.save();
                self.tags_changed = true;
                self.sync_tag_filters();
                self.sync_view_tags();
            }
            ChipAction::Remove => {
                self.tag_store.remove(index);
                self.tags_changed = true;
                self.sync_tag_filters();
                self.sync_view_tags();
            }
            ChipAction::Edit => {
                self.popup = Some(TagEditPopup::edit(&self.tag_store.tags[index]));
                return operation::focus(tag_ui::TAG_LABEL_INPUT);
            }
        }
        Task::none()
    }

    fn commit_popup(&mut self, popup: TagEditPopup) {
        match popup.editing {
            None => {
                let index = self.tag_store.add(
                    &popup.result_label(),
                    &popup.pattern,
                    popup.ignore_case,
                    popup.highlight,
                    popup.filter,
                    popup.color_index,
                    popup.text_color_index,
                );
                self.tag_store.tags[index].level = popup.level;
                self.tag_store.save();
            }
            Some(id) => {
                let Some(index) = self.tag_store.tags.iter().position(|t| t.id == id) else {
                    return;
                };
                {
                    let tag = &mut self.tag_store.tags[index];
                    tag.label = popup.result_label();
                    tag.pattern = popup.pattern.clone();
                    tag.ignore_case = popup.ignore_case;
                    tag.highlight_on = popup.highlight;
                    tag.filter_on = popup.filter;
                    tag.color_index = popup.color_index;
                    tag.text_color_index = popup.text_color_index;
                    tag.level = popup.level;
                    tag.rebuild_matcher();
                }
                self.tag_store.save();

                // The pattern may have changed, so drop the old index; the sync restarts it.
                if self.tag_indexes.contains_key(&id) {
                    self.stop_tag_filter(id);
                    self.rebuild_union();
                }
            }
        }
        self.tags_changed = true;
        self.sync_tag_filters();
        self.sync_view_tags();
    }

    /// Moves the tag open in the editor one place left or right, which is how levels are ranked.
    fn move_tag(&mut self, delta: i64) {
        let Some(id) = self.popup.as_ref().and_then(|p| p.editing) else {
            return;
        };
        let Some(index) = self.tag_store.tags.iter().position(|t| t.id == id) else {
            return;
        };
        if self.tag_store.move_tag(index, delta).is_some() {
            self.tag_store.save();
            self.tags_changed = true;
            self.sync_tag_filters();
            self.sync_view_tags();
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.apply(message);
        self.sync_diff();
        self.refresh_status();
        self.sync_stats();
        self.sync_notes();
        self.busy |= self.stats_busy() || self.notes_pin.is_some() || self.diff_job.is_some();
        task
    }

    fn apply(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SearchChanged(search) => {
                self.search = search;
                // Debounce typing, same 250 ms the C# `_typeTimer` used.
                self.type_deadline = Some(Instant::now() + Duration::from_millis(250));
            }
            Message::SearchSubmitted => {
                if self.type_deadline.is_some() {
                    self.start_match_scan();
                }
                self.jump_match(if self.modifiers.shift() { -1 } else { 1 });
            }
            Message::ToggleRegex => {
                self.regex_on = !self.regex_on;
                self.start_match_scan();
            }
            Message::ToggleCase => {
                self.case_on = !self.case_on;
                self.start_match_scan();
            }
            Message::ToggleFilter => {
                self.filter_on = !self.filter_on;
                self.recompute_active_filter();
            }
            Message::ToggleExclude => {
                // Excluding only means something through the filter, so turning it on implies it.
                self.exclude_on = !self.exclude_on;
                if self.exclude_on {
                    self.filter_on = true;
                }
                self.recompute_active_filter();
            }
            Message::TogglePatterns => self.toggle_semantic_zoom(),
            Message::ToggleWrap => self.view.toggle_wrap_all(),
            Message::ToggleColumns => {
                if self.columns_wanted {
                    self.columns_off();
                } else if self.file.is_some() {
                    let dialog = ColumnsDialog::new(&self.column_settings);
                    let auto = dialog.auto;
                    self.columns_dialog = Some(dialog);
                    if !auto {
                        return operation::focus(tag_ui::COLUMNS_INPUT);
                    }
                }
            }
            Message::Columns(message) => return self.apply_columns_message(message),
            Message::GoTop => self.view.go_to_start(),
            Message::GoBottom => self.view.go_to_end(),
            Message::OpenDialog => self.open_dialog(None),
            Message::OpenRecent(path) => self.load_file(&path.to_string_lossy()),
            Message::BrowseFolder(folder) => self.open_dialog(Some(&folder)),
            Message::Reload => self.reload(),
            Message::NextChange => self.next_change(),
            Message::ToggleChangesFilter => {
                self.changes_filter = !self.changes_filter;
                self.recompute_active_filter();
            }
            Message::ClearChanges => {
                self.changes = Arc::default();
                self.show_changes();
                if self.changes_filter {
                    self.changes_filter = false;
                    self.recompute_active_filter();
                }
            }
            Message::CheckDisk => {
                self.check_disk();
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.at.elapsed() >= NOTICE_FOR)
                {
                    self.notice = None;
                }
            }
            Message::NewTag => {
                self.popup = Some(TagEditPopup::new_tag(
                    self.tag_store.tags.len() % TAG_PALETTE.len(),
                ));
                return operation::focus(tag_ui::TAG_LABEL_INPUT);
            }
            Message::Chip(id, action) => return self.apply_chip_action(id, action),
            Message::Popup(message) => {
                let Some(popup) = self.popup.as_mut() else {
                    return Task::none();
                };
                match popup.update(message) {
                    PopupOutcome::Open => {}
                    PopupOutcome::Accepted => {
                        let popup = self.popup.take().expect("checked above");
                        self.commit_popup(popup);
                    }
                    PopupOutcome::Cancelled => self.popup = None,
                    PopupOutcome::Move(delta) => self.move_tag(delta),
                }
            }
            Message::Goto(message) => match message {
                GotoMessage::Input(text) => self.goto_text = text,
                GotoMessage::Go => {
                    if let (Ok(n), Some(file)) =
                        (self.goto_text.trim().parse::<i64>(), self.file.clone())
                    {
                        let last = file.line_count() as i64 - 1;
                        self.view.go_to_actual((n - 1).clamp(0, last.max(0)));
                    }
                    self.close_goto();
                }
                GotoMessage::Cancel => self.close_goto(),
            },
            Message::ToggleStats => {
                self.side_panel = match self.side_panel {
                    Some(SidePanel::Stats) => None,
                    _ => Some(SidePanel::Stats),
                };
            }
            Message::Stats(message) => self.apply_stats_message(message),
            Message::ToggleNotes => {
                self.side_panel = match self.side_panel {
                    Some(SidePanel::Notes) => None,
                    _ => Some(SidePanel::Notes),
                };
            }
            Message::Notes(message) => self.apply_notes_message(message),
            Message::OpenCopy => return self.open_copy_dialog(),
            Message::Copy(message) => return self.apply_copy_message(message),
            Message::View(message) => match message {
                ViewMessage::ScrollRows(delta) => self.view.scroll_by(delta),
                ViewMessage::ScrollCols(delta) => self.view.scroll_cols_by(delta),
                ViewMessage::ScrollTo(top) => self.view.scroll_to(top),
                ViewMessage::ScrollColTo(col) => self.view.scroll_cols_to(col),
                ViewMessage::Select(row) if self.modifiers.shift() => {
                    self.view.extend_selection(row)
                }
                ViewMessage::Select(row) => self.view.select(row),
                ViewMessage::DragText(from, to) => self.view.set_text_selection(from, to),
                ViewMessage::NoteMarker(row) => {
                    self.view.select(row);
                    self.focus_note_line(self.view.line_at(row));
                }
                ViewMessage::Activate(row) => {
                    self.view.select(row);
                    if self.view.mode() == LogViewMode::Templates {
                        self.zoom_into_selected_template();
                    } else {
                        self.view.toggle_wrap_line(row);
                    }
                }
            },
            Message::KeyPressed {
                key,
                modifiers,
                captured,
            } => return self.handle_key(key, modifiers, captured),
            Message::Tick => {
                if self.type_deadline.is_some_and(|d| Instant::now() >= d) {
                    self.start_match_scan();
                }
            }
            Message::ToggleTheme => {}
        }
        Task::none()
    }

    fn close_goto(&mut self) {
        self.goto_open = false;
        self.goto_text.clear();
    }

    /// Global shortcuts first (the C# `ProcessCmdKey`), then whatever the log surface owns — but
    /// only for keys no text input claimed, which is how WinForms split it from
    /// `LogView.OnKeyDown`.
    fn handle_key(
        &mut self,
        key: Key,
        modifiers: keyboard::Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if self.popup.is_some() || self.goto_open || self.copy_open || self.columns_dialog.is_some()
        {
            if key == Key::Named(Named::Escape) {
                self.popup = None;
                self.close_goto();
                self.close_copy();
                self.columns_dialog = None;
            }
            return Task::none();
        }

        if modifiers.command() {
            if let Key::Character(c) = &key {
                match c.to_lowercase().as_str() {
                    "c" if modifiers.shift() => return self.open_copy_dialog(),
                    "n" if modifiers.shift() => {
                        self.add_note_for_selection();
                        return Task::none();
                    }
                    "o" => {
                        self.open_dialog(None);
                        return Task::none();
                    }
                    "f" => return self.focus_search(),
                    "h" => {
                        self.filter_on = !self.filter_on;
                        self.recompute_active_filter();
                        return Task::none();
                    }
                    "g" if self.file.is_some() => {
                        self.goto_open = true;
                        return operation::focus(tag_ui::GOTO_INPUT);
                    }
                    "m" => {
                        self.toggle_semantic_zoom();
                        return Task::none();
                    }
                    _ => {}
                }
            }
        }

        if modifiers.alt() && matches!(&key, Key::Character(c) if c.eq_ignore_ascii_case("z")) {
            self.view.toggle_wrap_all();
            return Task::none();
        }

        match key {
            Key::Named(Named::F3) => {
                self.jump_match(if modifiers.shift() { -1 } else { 1 });
                return Task::none();
            }
            Key::Named(Named::F5) => {
                self.reload();
                return Task::none();
            }
            // Escape inside the search box (it unfocuses itself): clear it, or step out when
            // there is nothing to clear — the C# `Search_KeyDown`.
            Key::Named(Named::Escape) if captured => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.start_match_scan();
                } else {
                    self.step_out_one_zoom_level();
                }
                return Task::none();
            }
            _ if captured => return Task::none(),
            _ => {}
        }

        let outcome = self.view.handle_key(&key, modifiers);
        if outcome.template_activated {
            self.zoom_into_selected_template();
        }
        if outcome.escape_pressed {
            self.step_out_one_zoom_level();
        }
        if outcome.copy_requested {
            return self.copy(CopyKind::Selection);
        }
        Task::none()
    }

    // -------------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let surface: Element<'_, Message> = if self.file.is_some() {
            canvas(&self.view)
                .width(Length::Fill)
                .height(Length::Fill)
                .map(Message::View)
                .boxed()
        } else {
            self.empty_state()
        };

        let mut layout = column![self.toolbar(), rule(), self.tag_bar(), rule()];
        if let Some(crumb) = self.breadcrumb() {
            layout = layout.push(crumb).push(rule());
        }
        let surface = container(surface)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::log_surface);
        let body: Element<'_, Message> = match self.side_panel {
            Some(SidePanel::Stats) if self.file.is_some() => row![
                surface,
                vertical_rule(),
                stats_panel(self.tag_stats(), self.view.filter_on()).map(Message::Stats),
            ]
            .boxed(),
            Some(SidePanel::Notes) if self.file.is_some() => {
                row![surface, vertical_rule(), self.notes_view()].boxed()
            }
            _ => surface.boxed(),
        };
        let layout = layout.push(body).push(rule()).push(self.status_bar());

        let base = container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::log_surface);

        if let Some(popup) = &self.popup {
            stack![base, modal(popup.view().map(Message::Popup))].boxed()
        } else if self.goto_open {
            let lines = self.file.as_ref().map_or(0, |f| f.line_count());
            stack![
                base,
                modal(goto_view(&self.goto_text, lines).map(Message::Goto))
            ]
            .boxed()
        } else if self.copy_open {
            let lines = self.file.as_ref().map_or(0, |f| f.line_count());
            let in_lines = self.view.mode() == LogViewMode::Lines;
            let line = self.view.selected_actual_line();
            let choices = CopyChoices {
                has_text: self.view.has_text_selection(),
                line: (in_lines && line >= 0).then_some(line + 1),
                lines: self
                    .view
                    .selected_line_range()
                    .map_or(0, |(first, last)| last - first + 1),
            };
            let dialog = copy_view(choices, &self.copy_range, self.copy_error.as_deref(), lines);
            stack![base, modal(dialog.map(Message::Copy))].boxed()
        } else if let Some(dialog) = &self.columns_dialog {
            stack![base, modal(dialog.view().map(Message::Columns))].boxed()
        } else {
            base.boxed()
        }
    }

    fn toolbar(&self) -> Element<'_, Message> {
        responsive(move |size| self.toolbar_at(size.width < NARROW_TOOLBAR))
            .height(Length::Shrink)
            .boxed()
    }

    /// `narrow` (a split pane) moves the search box and its options onto a second row, where it
    /// keeps a usable width instead of being squeezed out by the buttons.
    fn toolbar_at(&self, narrow: bool) -> Element<'_, Message> {
        let has_file = self.file.is_some();

        let open = with_tip(
            button(icon(icons::FOLDER, 16.0, theme::palette().text_dim))
                .padding(8)
                .style(theme::ghost)
                .on_press(Message::OpenDialog),
            "Open file  (Ctrl+O)",
            tooltip::Position::Bottom,
        );

        let jump = |bytes: &'static [u8], message: Message, tip: &'static str| {
            with_tip(
                button(icon(bytes, 16.0, theme::palette().text_dim))
                    .padding(8)
                    .style(theme::ghost)
                    .on_press_maybe(has_file.then_some(message)),
                tip,
                tooltip::Position::Bottom,
            )
        };
        let reload = jump(icons::RELOAD, Message::Reload, "Reload the file  (F5)");
        let copy = jump(icons::COPY, Message::OpenCopy, "Copy…  (Ctrl+Shift+C)");
        let top = jump(icons::TO_TOP, Message::GoTop, "Go to top  (Ctrl+Home)");
        let bottom = jump(
            icons::TO_BOTTOM,
            Message::GoBottom,
            "Go to bottom  (Ctrl+End)",
        );

        let placeholder = if self.view.mode() == LogViewMode::Templates {
            "Filter patterns…"
        } else if self.zoom.is_some() {
            "Search this pattern, or filter slots: slot2 > 2000 AND slot1 = warn"
        } else {
            "Search…"
        };
        // Leave room on the right for the match count drawn over the input.
        let count_room = self.count_text.chars().count() as f32 * 7.0 + 16.0;
        let input = text_input(placeholder, &self.search)
            .id(self.search_input.clone())
            .on_input(Message::SearchChanged)
            .on_submit(Message::SearchSubmitted)
            .font(theme::MONO)
            .size(theme::MONO_SIZE)
            .padding(Padding {
                top: 8.0,
                bottom: 8.0,
                left: 36.0,
                right: count_room,
            })
            .style(theme::input);
        let count = text(&self.count_text)
            .size(theme::SMALL_SIZE)
            .font(theme::UI_SMALL)
            .color(if self.count_error {
                theme::palette().error
            } else {
                theme::palette().text_dim
            });
        let search = stack![
            input,
            container(icon(icons::SEARCH, 15.0, theme::palette().text_faint))
                .padding([0, 12])
                .height(Length::Fill)
                .align_y(Alignment::Center),
            container(count)
                .padding([0, 12])
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::End)
                .align_y(Alignment::Center),
        ]
        .width(Length::Fill);

        let toggle =
            |label: &'static str, mono: bool, on: bool, message: Message, tip: &'static str| {
                let label = text(label)
                    .size(if mono {
                        theme::MONO_SIZE
                    } else {
                        theme::UI_SIZE
                    })
                    .font(if mono { theme::MONO } else { theme::UI })
                    .height(Length::Fill)
                    .align_y(Alignment::Center);
                with_tip(
                    button(label)
                        .height(30)
                        .padding([0, 10])
                        .style(theme::toggle(on))
                        .on_press(message),
                    tip,
                    tooltip::Position::Bottom,
                )
            };
        let options = container(
            row![
                toggle(
                    ".*",
                    true,
                    self.regex_on,
                    Message::ToggleRegex,
                    "Regular expression"
                ),
                toggle("Aa", true, self.case_on, Message::ToggleCase, "Match case"),
                toggle(
                    "Filter",
                    false,
                    self.filter_on,
                    Message::ToggleFilter,
                    "Show only matching lines  (Ctrl+H)"
                ),
                toggle(
                    "Exclude",
                    false,
                    self.exclude_on,
                    Message::ToggleExclude,
                    "Filter out matching lines instead of keeping them"
                ),
            ]
            .spacing(2),
        )
        .padding(2)
        .style(theme::segmented);

        let patterns_on = self.view.mode() == LogViewMode::Templates || self.zoom.is_some();
        let patterns = with_tip(
            button(
                row![
                    icon(
                        icons::LAYERS,
                        14.0,
                        if patterns_on {
                            theme::palette().accent
                        } else {
                            theme::palette().text_dim
                        }
                    ),
                    text("Patterns").size(theme::UI_SIZE),
                ]
                .spacing(7)
                .height(Length::Fill)
                .align_y(Alignment::Center),
            )
            .height(34)
            .padding([0, 12])
            .style(theme::toggle(patterns_on))
            .on_press_maybe(has_file.then_some(Message::TogglePatterns)),
            "Group similar lines into patterns  (Ctrl+M)",
            tooltip::Position::Bottom,
        );

        let wrap = with_tip(
            button(
                text("Wrap")
                    .size(theme::UI_SIZE)
                    .height(Length::Fill)
                    .align_y(Alignment::Center),
            )
            .height(34)
            .padding([0, 12])
            .style(theme::toggle(self.view.wrap_all()))
            .on_press_maybe(has_file.then_some(Message::ToggleWrap)),
            "Wrap long lines  (Alt+Z)\nEnter or double-click wraps just the selected line",
            tooltip::Position::Bottom,
        );

        let mut tools = row![open, reload].spacing(8).align_y(Alignment::Center);
        if let Some((label, tip)) = match self.disk_state {
            DiskState::Unchanged => None,
            DiskState::Changed => Some((
                "File changed",
                "The file changed on disk. Click to reload  (F5)",
            )),
            DiskState::Missing => Some(("File missing", "The file was moved or deleted")),
        } {
            tools = tools.push(with_tip(
                button(
                    container(text(label).size(theme::SMALL_SIZE).font(theme::UI_SMALL))
                        .padding([3, 9])
                        .style(theme::pill(theme::palette().warn)),
                )
                .padding(0)
                .style(theme::ghost)
                .on_press(Message::Reload),
                tip,
                tooltip::Position::Bottom,
            ));
        }

        if !self.changes.is_empty() {
            tools = tools.push(self.changes_badge());
        }

        let columns = with_tip(
            button(
                text("Columns")
                    .size(theme::UI_SIZE)
                    .height(Length::Fill)
                    .align_y(Alignment::Center),
            )
            .height(34)
            .padding([0, 12])
            .style(theme::toggle(self.columns_wanted))
            .on_press_maybe(has_file.then_some(Message::ToggleColumns)),
            "Lay structured lines out in columns (time, level, …, JSON keys, or split on a delimiter)",
            tooltip::Position::Bottom,
        );

        let (theme_icon, theme_tip) = if theme::is_light() {
            (icons::MOON, "Switch to the dark theme")
        } else {
            (icons::SUN, "Switch to the light theme")
        };
        let theme_toggle = with_tip(
            button(icon(theme_icon, 16.0, theme::palette().text_dim))
                .padding(8)
                .style(theme::ghost)
                .on_press(Message::ToggleTheme),
            theme_tip,
            tooltip::Position::Bottom,
        );

        let content: Element<'_, Message> = if narrow {
            column![
                row![
                    tools,
                    top,
                    bottom,
                    copy,
                    space().width(Length::Fill),
                    wrap,
                    columns,
                    patterns,
                    theme_toggle
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![search, options].spacing(8).align_y(Alignment::Center),
            ]
            .spacing(8)
            .boxed()
        } else {
            row![
                tools,
                top,
                bottom,
                copy,
                search,
                options,
                wrap,
                columns,
                patterns,
                theme_toggle
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .boxed()
        };
        container(content)
            .padding([10, 12])
            .width(Length::Fill)
            .style(theme::surface)
            .boxed()
    }

    /// `+N -M  F x`: how many lines changed on disk. The count steps through the changes, F shows
    /// only them, x clears the marks.
    fn changes_badge(&self) -> Element<'_, Message> {
        let color = theme::palette().diff_added;
        let label = format!(
            "+{} −{}",
            thousands(self.changes.changed_lines() as u64),
            thousands(self.changes.removed_lines() as u64)
        );
        let count = button(text(label).size(theme::SMALL_SIZE).font(theme::UI_SMALL))
            .padding([3, 4])
            .style(theme::chip_label)
            .on_press(Message::NextChange);
        // Same glyph-baseline nudge as the tag chips' switches.
        let filter = button(text("F").size(11).font(theme::MONO).center())
            .width(20)
            .height(20)
            .padding(Padding {
                top: 1.0,
                ..Padding::ZERO
            })
            .style(theme::chip_switch(self.changes_filter, false, color))
            .on_press(Message::ToggleChangesFilter);
        let clear = button(text("×").size(14).center())
            .width(20)
            .height(20)
            .padding(0)
            .style(theme::chip_remove)
            .on_press(Message::ClearChanges);
        container(
            row![
                with_tip(
                    count,
                    "Lines changed on disk since the file was opened: new or rewritten, and removed.\nClick to go to the next change",
                    tooltip::Position::Bottom,
                ),
                with_tip(filter, "Show only the changed lines", tooltip::Position::Bottom),
                with_tip(clear, "Clear the change marks", tooltip::Position::Bottom),
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        )
        .padding([2, 4])
        .style(theme::chip(color))
        .boxed()
    }

    fn tag_bar(&self) -> Element<'_, Message> {
        let mut chips = row![text("TAGS").size(10.5).color(theme::palette().text_faint)]
            .spacing(6)
            .align_y(Alignment::Center)
            .padding([0, 12]);
        let filtering = self.tag_store.filtering_ids();
        for tag in &self.tag_store.tags {
            chips = chips.push(tag_chip(tag, filtering.contains(&tag.id), Message::Chip));
        }
        chips = chips.push(with_tip(
            button(
                row![
                    icon(icons::PLUS, 12.0, theme::palette().text_dim),
                    text("New tag")
                        .size(theme::SMALL_SIZE)
                        .font(theme::UI_SMALL),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([4, 10])
            .style(theme::secondary)
            .on_press(Message::NewTag),
            "Add a saved highlight / filter",
            tooltip::Position::Bottom,
        ));
        let stats_on = self.side_panel == Some(SidePanel::Stats);
        chips = chips.push(with_tip(
            button(text("Stats").size(theme::SMALL_SIZE).font(theme::UI_SMALL))
                .padding([4, 10])
                .style(theme::toggle(stats_on))
                .on_press_maybe(self.file.is_some().then_some(Message::ToggleStats)),
            "How often each tag matches, and where in the file",
            tooltip::Position::Bottom,
        ));
        let notes_on = self.side_panel == Some(SidePanel::Notes);
        let note_count = self.notes.line_notes.len() + self.notes.pattern_notes.len();
        chips = chips.push(with_tip(
            button(
                text(if note_count > 0 {
                    format!("Notes ({note_count})")
                } else {
                    "Notes".to_string()
                })
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL),
            )
            .padding([4, 10])
            .style(theme::toggle(notes_on))
            .on_press_maybe(self.file.is_some().then_some(Message::ToggleNotes)),
            "Notes on lines and patterns  (Ctrl+Shift+N adds one)",
            tooltip::Position::Bottom,
        ));

        container(scrollable(chips).direction(Direction::Horizontal(
            Scrollbar::new().width(0).scroller_width(0),
        )))
        // Fixed, so adding the first chip (a touch taller than the button) doesn't nudge the log.
        .height(42)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .style(theme::surface)
        .boxed()
    }

    /// Names the zoom level when semantic zoom is active, with the keys that move between levels.
    fn breadcrumb(&self) -> Option<Element<'_, Message>> {
        let pill = |label: &'static str| {
            container(text(label).size(10.5))
                .padding([2, 8])
                .style(theme::pill(theme::palette().accent))
        };
        let hint = |hint: &'static str| {
            text(hint)
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().text_faint)
        };

        let content = if self.view.mode() == LogViewMode::Templates {
            let count = self.templates.as_ref().map_or(0, |t| t.template_count());
            row![
                pill("PATTERNS"),
                text(format!(
                    "{} patterns, most frequent first",
                    thousands(count as u64)
                ))
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().text_dim),
                space().width(Length::Fill),
                hint("↑↓ select  ·  Enter show lines  ·  Esc back"),
            ]
        } else {
            let zoom = self.zoom.as_ref()?;
            row![
                pill("PATTERN"),
                text(zoom.render())
                    .font(theme::MONO)
                    .size(theme::SMALL_SIZE)
                    .wrapping(text::Wrapping::None),
                space().width(Length::Fill),
                hint("Esc back to patterns"),
            ]
        };

        Some(
            container(content.spacing(10).align_y(Alignment::Center))
                .padding([6, 12])
                .width(Length::Fill)
                .clip(true)
                .style(theme::breadcrumb)
                .boxed(),
        )
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let dot = if self.busy {
            theme::palette().accent
        } else if self.file.is_some() {
            theme::palette().info
        } else {
            theme::palette().text_faint
        };

        let mut bar = row!(
            container(space()).width(7).height(7).style(theme::dot(dot)),
            text(&self.status_left)
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().text_dim)
                .wrapping(text::Wrapping::None),
        )
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(notice) = &self.notice {
            if notice.at.elapsed() < NOTICE_FOR {
                bar = bar.push(
                    text(&notice.text)
                        .size(theme::SMALL_SIZE)
                        .font(theme::UI_SMALL)
                        .color(if notice.error {
                            theme::palette().error
                        } else {
                            theme::palette().accent
                        })
                        .wrapping(text::Wrapping::None)
                        .boxed(),
                );
            }
        }
        bar = bar.push(space().width(Length::Fill).boxed());

        for (keys, label) in HINTS {
            bar = bar.push(
                row![
                    // A whole-pixel line height keeps the box from rounding to an odd
                    // height, and the glyph baseline snaps up a pixel, so the extra
                    // pixel of padding goes on top to land the keys in the middle.
                    container(
                        text(keys)
                            .font(theme::MONO)
                            .size(10.5)
                            .line_height(text::LineHeight::Absolute(12.0.into()))
                            .wrapping(text::Wrapping::None),
                    )
                    .padding(Padding {
                        top: 2.0,
                        bottom: 0.0,
                        left: 5.0,
                        right: 5.0,
                    })
                        .style(theme::kbd),
                    text(label)
                        .size(theme::SMALL_SIZE)
                        .font(theme::UI_SMALL)
                        .color(theme::palette().text_faint)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(5)
                .align_y(Alignment::Center)
                .boxed(),
            );
        }

        container(bar.spacing(12))
            .padding([5, 12])
            .width(Length::Fill)
            .clip(true)
            .style(theme::surface)
            .boxed()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        let recent = self
            .recent
            .paths
            .iter()
            .map(|path| {
                start_entry(
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    path.to_str().unwrap_or_default(),
                    Message::OpenRecent(path.clone()),
                )
            })
            .collect();
        let folders = self
            .log_folders
            .iter()
            .map(|(label, path)| {
                start_entry(
                    label.to_string(),
                    path.display().to_string(),
                    path.to_str().unwrap_or_default(),
                    Message::BrowseFolder(path.clone()),
                )
            })
            .collect();

        let hero = column![
            container(icon(icons::FILE, 40.0, theme::palette().text_faint))
                .padding(18)
                .style(theme::segmented),
            space().height(4),
            text("No log file open").size(18),
            text("Drop a file anywhere in this window, or open one to start.")
                .size(theme::UI_SIZE)
                .color(theme::palette().text_dim),
            space().height(6),
            button(
                row![
                    icon(icons::FOLDER, 14.0, theme::palette().on_accent),
                    text("Open file…").size(theme::UI_SIZE),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([8, 16])
            .style(theme::primary)
            .on_press(Message::OpenDialog),
            text("or press Ctrl+O")
                .size(theme::SMALL_SIZE)
                .font(theme::UI_SMALL)
                .color(theme::palette().text_faint),
        ]
        .spacing(10)
        .align_x(Alignment::Center);

        let lists = row![
            start_list("Recent files", recent, "Files you open will show up here."),
            start_list(
                "Log locations",
                folders,
                "None of the usual log folders exist."
            ),
        ]
        .spacing(16)
        // A split pane may be too narrow for both lists side by side.
        .wrap()
        .vertical_spacing(16)
        .align_x(Alignment::Center);

        // Top-aligned rather than centred: a full recent list is taller than the default window,
        // and scrollable content can't fill the height to centre within it.
        container(
            scrollable(
                column![hero, space().height(18), lists]
                    .width(Length::Fill)
                    .spacing(10)
                    .padding(Padding {
                        top: 48.0,
                        bottom: 24.0,
                        left: 16.0,
                        right: 16.0,
                    })
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .style(theme::log_surface)
        .boxed()
    }
}

/// One of the start screen's lists, under a small heading.
fn start_list<'a>(
    title: &'a str,
    entries: Vec<Element<'a, Message>>,
    when_empty: &'a str,
) -> Element<'a, Message> {
    let heading = container(
        text(title)
            .size(theme::SMALL_SIZE)
            .font(theme::UI_SMALL)
            .color(theme::palette().text_dim),
    )
    .padding([2, 8]);
    let mut list = column![heading].spacing(2);
    if entries.is_empty() {
        list = list.push(
            container(
                text(when_empty)
                    .size(theme::SMALL_SIZE)
                    .font(theme::UI_SMALL)
                    .color(theme::palette().text_faint),
            )
            .padding([5, 8])
            .boxed(),
        );
    } else {
        list = list.extend(entries);
    }
    container(list)
        .width(320)
        .padding(8)
        .style(theme::segmented)
        .boxed()
}

/// A start screen row: a name over its folder, with the whole path on hover.
fn start_entry<'a>(
    name: String,
    detail: String,
    path: &'a str,
    message: Message,
) -> Element<'a, Message> {
    with_tip(
        button(
            column![
                text(name)
                    .size(theme::UI_SIZE)
                    .color(theme::palette().text)
                    .wrapping(text::Wrapping::None),
                text(detail)
                    .size(theme::SMALL_SIZE)
                    .font(theme::UI_SMALL)
                    .color(theme::palette().text_faint)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(1),
        )
        .width(Length::Fill)
        .padding([5, 8])
        .clip(true)
        .style(theme::ghost)
        .on_press(message),
        path,
        tooltip::Position::Bottom,
    )
}

/// Folders where Windows and common services keep their logs, those that exist on this machine.
fn log_folders() -> Vec<(&'static str, PathBuf)> {
    let windows =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let program_data = std::env::var_os("ProgramData")
        .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let candidates = [
        ("Temp", Some(std::env::temp_dir())),
        ("Local AppData", dirs::data_local_dir()),
        ("Roaming AppData", dirs::config_dir()),
        ("ProgramData", Some(program_data)),
        ("Windows logs", Some(windows.join("Logs"))),
        (
            "System32 log files",
            Some(windows.join(r"System32\LogFiles")),
        ),
        ("IIS logs", Some(PathBuf::from(r"C:\inetpub\logs\LogFiles"))),
    ];
    candidates
        .into_iter()
        .filter_map(|(label, path)| path.filter(|p| p.is_dir()).map(|p| (label, p)))
        .collect()
}

pub fn icon<'a>(bytes: &'static [u8], size: f32, color: Color) -> svg::Svg<'a> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(theme::icon(color))
}

/// A one-pixel vertical divider.
fn vertical_rule<'a>() -> Element<'a, Message> {
    container(space())
        .width(1)
        .height(Length::Fill)
        .style(theme::divider)
        .boxed()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f64 / whole as f64
    }
}

/// A one-pixel horizontal divider.
fn rule<'a>() -> Element<'a, Message> {
    container(space())
        .width(Length::Fill)
        .height(1)
        .style(theme::divider)
        .boxed()
}
