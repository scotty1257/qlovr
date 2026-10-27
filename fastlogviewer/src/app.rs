//! The window: toolbar, tag row, log surface, status strip, and the worker threads that feed
//! them.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use fastlogviewer_core::{
    join_lines, parse_line_range, CancelToken, ComplementIndex, FilterIndex, IntersectionIndex,
    Joined, LogFile, LogTemplate, LogViewMode, MatchIndex, Matcher, SlotFilterIndex, SlotQuery,
    TagStore, TemplateIndex, UnionFilterIndex, COPY_LIMIT_BYTES, TAG_PALETTE,
};
use iced::keyboard::{self, key::Named, Key};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, canvas, center, column, container, operation, row, scrollable, space, stack, svg, text,
    text_input, tooltip, Id,
};
use iced::{event, window, Alignment, Color, Element, Event, Length, Padding, Subscription, Task};
use uuid::Uuid;

use crate::format::{thousands, thousands_1dp};
use crate::log_view::{LogView, ViewMessage};
use crate::tag_ui::{
    self, copy_view, goto_view, modal, tag_chip, with_tip, ChipAction, CopyChoices, CopyMessage,
    GotoMessage, PopupMessage, PopupOutcome, TagEditPopup,
};
use crate::theme::{self, icons};
use crate::win;

const SEARCH_INPUT: Id = Id::new("search");

const HINTS: [(&str, &str); 5] = [
    ("Ctrl O", "open"),
    ("Ctrl F", "search"),
    ("F3", "next"),
    ("Ctrl H", "filter"),
    ("Ctrl M", "patterns"),
];

/// How often the open file's size and modified time are compared with what was loaded.
const DISK_CHECK: Duration = Duration::from_secs(2);
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
    GoTop,
    GoBottom,
    OpenDialog,
    Reload,
    CheckDisk,
    NewTag,
    Chip(Uuid, ChipAction),
    Popup(PopupMessage),
    Goto(GotoMessage),
    OpenCopy,
    Copy(CopyMessage),
    View(ViewMessage),
    KeyPressed {
        key: Key,
        modifiers: keyboard::Modifiers,
        /// A focused text input already handled it.
        captured: bool,
    },
    ModifiersChanged(keyboard::Modifiers),
    FileDropped(PathBuf),
    Tick,
    /// The HWND is only known once the window exists; the title bar is tinted from there. Opening
    /// can race the subscription that would see it, so the first focus counts too.
    WindowShown(window::Id),
    WindowReady(u64),
}

pub struct App {
    view: LogView,
    file: Option<Arc<LogFile>>,
    file_ct: Option<CancelToken>,
    title: String,
    file_stamp: Option<FileStamp>,
    disk_state: DiskState,
    pending_restore: Option<Restore>,
    notice: Option<Notice>,

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
    chrome_applied: bool,
}

impl App {
    pub fn new(initial_file: Option<String>) -> (Self, Task<Message>) {
        let mut app = App {
            view: LogView::new(),
            file: None,
            file_ct: None,
            title: "FastLogViewer".to_string(),
            file_stamp: None,
            disk_state: DiskState::Unchanged,
            pending_restore: None,
            notice: None,
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
            chrome_applied: false,
        };

        app.tag_store.load();
        app.sync_view_tags();

        if let Some(path) = initial_file {
            if Path::new(&path).is_file() {
                app.load_file(&path);
            }
        }
        app.refresh_status();
        (app, Task::none())
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }

    fn sync_view_tags(&mut self) {
        self.view.set_tags(self.tag_store.tags.clone());
    }

    // ------------------------------------------------------------ file loading

    pub fn load_file(&mut self, path: &str) {
        self.release_current();
        self.pending_restore = None;
        self.notice = None;
        self.disk_state = DiskState::Unchanged;
        // Taken before opening, so a write that lands in between shows as a change, not a miss.
        self.file_stamp = FileStamp::of(Path::new(path));

        let file = match LogFile::open(path) {
            Ok(f) => Arc::new(f),
            Err(e) => {
                rfd::MessageDialog::new()
                    .set_title("Cannot open file")
                    .set_description(e.to_string())
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                return;
            }
        };

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
        // In patterns, or zoomed into one, the search box holds a pattern or slot filter that
        // means nothing to plain lines.
        if self.view.mode() == LogViewMode::Templates || self.zoom.is_some() {
            self.search.clear();
        }
        self.load_file(&path.to_string_lossy());
        if self.file.is_some() {
            self.pending_restore = Some(restore);
        }
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
        self.cancel_match_scan();
        self.cancel_all_tag_filters();
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

    fn open_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Log files", &["log", "txt"])
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            self.load_file(&path.to_string_lossy());
        }
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
        if self.tag_indexes.values().any(|i| !i.is_complete()) {
            busy = true;
        }

        let ad_hoc_complete = self.matches.as_ref().is_some_and(|m| m.is_complete());
        let union_complete = self.tag_union.as_ref().is_some_and(|u| u.is_complete());
        let slot_complete = self.slot_filter.as_ref().is_some_and(|s| s.is_complete());
        if ad_hoc_complete != self.last_adhoc_complete
            || union_complete != self.last_union_complete
            || slot_complete != self.last_slot_complete
        {
            self.last_adhoc_complete = ad_hoc_complete;
            self.last_union_complete = union_complete;
            self.last_slot_complete = slot_complete;
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
                self.sync_view_tags();
            }
            ChipAction::ToggleFilter => {
                self.tag_store.tags[index].filter_on = !self.tag_store.tags[index].filter_on;
                self.tag_store.save();
                self.sync_tag_filters();
                self.sync_view_tags();
            }
            ChipAction::Remove => {
                self.tag_store.remove(index);
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
                    Some(popup.color_index),
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
            self.sync_tag_filters();
            self.sync_view_tags();
        }
    }

    // ------------------------------------------------------------------ update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.apply(message);
        self.refresh_status();
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
            Message::GoTop => self.view.go_to_start(),
            Message::GoBottom => self.view.go_to_end(),
            Message::OpenDialog => self.open_dialog(),
            Message::Reload => self.reload(),
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
            Message::ModifiersChanged(modifiers) => self.modifiers = modifiers,
            Message::FileDropped(path) => self.load_file(&path.to_string_lossy()),
            Message::Tick => {
                if self.type_deadline.is_some_and(|d| Instant::now() >= d) {
                    self.start_match_scan();
                }
            }
            Message::WindowShown(id) if !self.chrome_applied => {
                self.chrome_applied = true;
                return window::raw_id::<Message>(id).map(Message::WindowReady);
            }
            Message::WindowShown(_) => {}
            Message::WindowReady(handle) => {
                win::apply_chrome(handle, theme::SURFACE, theme::TEXT_DIM)
            }
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
        if self.popup.is_some() || self.goto_open || self.copy_open {
            if key == Key::Named(Named::Escape) {
                self.popup = None;
                self.close_goto();
                self.close_copy();
            }
            return Task::none();
        }

        if modifiers.command() {
            if let Key::Character(c) = &key {
                match c.to_lowercase().as_str() {
                    "c" if modifiers.shift() => return self.open_copy_dialog(),
                    "o" => {
                        self.open_dialog();
                        return Task::none();
                    }
                    "f" => {
                        return operation::focus(SEARCH_INPUT)
                            .chain(operation::select_all(SEARCH_INPUT));
                    }
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

    pub fn subscription(&self) -> Subscription<Message> {
        let events = event::listen_with(|event, status, window| match event {
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                Some(Message::KeyPressed {
                    key,
                    modifiers,
                    captured: status == event::Status::Captured,
                })
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::Opened { .. } | window::Event::Focused) => {
                Some(Message::WindowShown(window))
            }
            _ => None,
        });

        let mut subscriptions = vec![events];
        if self.busy || self.type_deadline.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick));
        }
        if self.file.is_some() {
            subscriptions.push(iced::time::every(DISK_CHECK).map(|_| Message::CheckDisk));
        }
        Subscription::batch(subscriptions)
    }

    // -------------------------------------------------------------------- view

    pub fn view(&self) -> Element<'_, Message> {
        let surface: Element<'_, Message> = if self.file.is_some() {
            Element::from(canvas(&self.view).width(Length::Fill).height(Length::Fill))
                .map(Message::View)
        } else {
            self.empty_state()
        };

        let mut layout = column![self.toolbar(), rule(), self.tag_bar(), rule()];
        if let Some(crumb) = self.breadcrumb() {
            layout = layout.push(crumb).push(rule());
        }
        let layout = layout
            .push(
                container(surface)
                    .height(Length::Fill)
                    .style(theme::log_surface),
            )
            .push(rule())
            .push(self.status_bar());

        let base = container(layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::log_surface);

        if let Some(popup) = &self.popup {
            stack![base, modal(popup.view().map(Message::Popup))].into()
        } else if self.goto_open {
            let lines = self.file.as_ref().map_or(0, |f| f.line_count());
            stack![
                base,
                modal(goto_view(&self.goto_text, lines).map(Message::Goto))
            ]
            .into()
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
            stack![base, modal(dialog.map(Message::Copy))].into()
        } else {
            base.into()
        }
    }

    fn toolbar(&self) -> Element<'_, Message> {
        let has_file = self.file.is_some();

        let open = with_tip(
            button(icon(icons::FOLDER, 16.0, theme::TEXT_DIM))
                .padding(8)
                .style(theme::ghost)
                .on_press(Message::OpenDialog),
            "Open file  (Ctrl+O)",
            tooltip::Position::Bottom,
        );

        let jump = |bytes: &'static [u8], message: Message, tip: &'static str| {
            with_tip(
                button(icon(bytes, 16.0, theme::TEXT_DIM))
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
            .id(SEARCH_INPUT)
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
            .color(if self.count_error {
                theme::ERROR
            } else {
                theme::TEXT_DIM
            });
        let search = stack![
            input,
            container(icon(icons::SEARCH, 15.0, theme::TEXT_FAINT))
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
                            theme::ACCENT
                        } else {
                            theme::TEXT_DIM
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
                    container(text(label).size(theme::SMALL_SIZE))
                        .padding([3, 9])
                        .style(theme::pill(theme::WARN)),
                )
                .padding(0)
                .style(theme::ghost)
                .on_press(Message::Reload),
                tip,
                tooltip::Position::Bottom,
            ));
        }

        container(
            row![tools, top, bottom, copy, search, options, wrap, patterns]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([10, 12])
        .width(Length::Fill)
        .style(theme::surface)
        .into()
    }

    fn tag_bar(&self) -> Element<'_, Message> {
        let mut chips = row![text("TAGS").size(10.5).color(theme::TEXT_FAINT)]
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
                    icon(icons::PLUS, 12.0, theme::TEXT_DIM),
                    text("New tag").size(theme::SMALL_SIZE),
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

        container(scrollable(chips).direction(Direction::Horizontal(
            Scrollbar::new().width(0).scroller_width(0),
        )))
        // Fixed, so adding the first chip (a touch taller than the button) doesn't nudge the log.
        .height(42)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .style(theme::surface)
        .into()
    }

    /// Names the zoom level when semantic zoom is active, with the keys that move between levels.
    fn breadcrumb(&self) -> Option<Element<'_, Message>> {
        let pill = |label: &'static str| {
            container(text(label).size(10.5))
                .padding([2, 8])
                .style(theme::pill(theme::ACCENT))
        };
        let hint = |hint: &'static str| text(hint).size(theme::SMALL_SIZE).color(theme::TEXT_FAINT);

        let content = if self.view.mode() == LogViewMode::Templates {
            let count = self.templates.as_ref().map_or(0, |t| t.template_count());
            row![
                pill("PATTERNS"),
                text(format!(
                    "{} patterns, most frequent first",
                    thousands(count as u64)
                ))
                .size(theme::SMALL_SIZE)
                .color(theme::TEXT_DIM),
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
                .into(),
        )
    }

    fn status_bar(&self) -> Element<'_, Message> {
        let dot = if self.busy {
            theme::ACCENT
        } else if self.file.is_some() {
            theme::INFO
        } else {
            theme::TEXT_FAINT
        };

        let mut bar = row!(
            container(space()).width(7).height(7).style(theme::dot(dot)),
            text(&self.status_left)
                .size(theme::SMALL_SIZE)
                .color(theme::TEXT_DIM)
                .wrapping(text::Wrapping::None),
        )
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(notice) = &self.notice {
            if notice.at.elapsed() < NOTICE_FOR {
                bar = bar.push(
                    text(&notice.text)
                        .size(theme::SMALL_SIZE)
                        .color(if notice.error {
                            theme::ERROR
                        } else {
                            theme::ACCENT
                        })
                        .wrapping(text::Wrapping::None),
                );
            }
        }
        bar = bar.push(space().width(Length::Fill));

        for (keys, label) in HINTS {
            bar = bar.push(
                row![
                    container(text(keys).font(theme::MONO).size(10.5))
                        .padding([1, 5])
                        .style(theme::kbd),
                    text(label).size(theme::SMALL_SIZE).color(theme::TEXT_FAINT),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            );
        }

        container(bar.spacing(12))
            .padding([5, 12])
            .width(Length::Fill)
            .clip(true)
            .style(theme::surface)
            .into()
    }

    fn empty_state(&self) -> Element<'_, Message> {
        center(
            column![
                container(icon(icons::FILE, 40.0, theme::TEXT_FAINT))
                    .padding(18)
                    .style(theme::segmented),
                space().height(4),
                text("No log file open").size(18),
                text("Drop a file anywhere in this window, or open one to start.")
                    .size(theme::UI_SIZE)
                    .color(theme::TEXT_DIM),
                space().height(6),
                button(
                    row![
                        icon(icons::FOLDER, 14.0, theme::ON_ACCENT),
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
                    .color(theme::TEXT_FAINT),
            ]
            .spacing(10)
            .align_x(Alignment::Center),
        )
        .style(theme::log_surface)
        .into()
    }
}

fn icon<'a>(bytes: &'static [u8], size: f32, color: Color) -> svg::Svg<'a> {
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .style(theme::icon(color))
}

/// A one-pixel horizontal divider.
fn rule<'a>() -> Element<'a, Message> {
    container(space())
        .width(Length::Fill)
        .height(1)
        .style(theme::divider)
        .into()
}
