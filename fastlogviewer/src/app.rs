//! The window: search bar, tag row, log surface, status strip, and the worker threads that feed
//! them.
//!
//! One structural difference from the C# `MainForm` worth naming. The original kept a `_stale` list
//! of worker threads and joined them before `Dispose`ing the memory map, because a scan still
//! running on a freed view would crash — and when a worker refused to stop in time it chose to leak
//! the mapping instead. Here every worker holds an `Arc<LogFile>`, so the mapping simply outlives
//! the last reader and unmaps itself. Cancellation stays (it stops useless work); the join
//! bookkeeping is gone because the hazard it guarded against cannot occur.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    pos2, vec2, Align2, CentralPanel, Context, Frame, Key, Modifiers, Rect, Sense, TextEdit,
    TopBottomPanel, Ui, ViewportCommand,
};
use fastlogviewer_core::{
    CancelToken, FilterIndex, IntersectionIndex, LogFile, LogTemplate, LogViewMode, MatchIndex,
    Matcher, SlotFilterIndex, SlotQuery, TagStore, TemplateIndex, UnionFilterIndex, TAG_PALETTE,
};
use uuid::Uuid;

use crate::log_view::LogView;
use crate::tag_ui::{self, ChipAction, PopupOutcome, TagEditPopup, CHIP_HEIGHT, CHIP_MARGIN};
use crate::theme;
use crate::widgets::{field_frame, status_strip, thousands, thousands_1dp, toggle_button};
use crate::win;

const HINTS: &str =
    "Ctrl+O open   ·   Ctrl+F search   ·   F3 next   ·   Ctrl+H filter   ·   Ctrl+M patterns";

const BAR_H: f32 = 46.0;
const TAG_ROW_H: f32 = 42.0;
const STATUS_H: f32 = 26.0;

/// Which index the ad-hoc/tag composition settled on, so the zoom layer can ask "is my base ready"
/// the way the C# did by comparing object references.
#[derive(PartialEq, Eq, Copy, Clone)]
enum Base {
    None,
    Intersection,
    AdHoc,
    TagUnion,
}

pub struct App {
    view: LogView,
    file: Option<Arc<LogFile>>,
    file_ct: Option<CancelToken>,
    title: String,

    search: String,
    regex_on: bool,
    case_on: bool,
    filter_on: bool,
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
    type_deadline: Option<Instant>,
    popup: Option<TagEditPopup>,
    goto_open: bool,
    goto_text: String,
    goto_focus: bool,
    search_focus: bool,
    chrome_applied: bool,
}

impl App {
    pub fn new(ctx: &Context, initial_file: Option<String>) -> Self {
        theme::install_fonts(ctx);

        let mut app = App {
            view: LogView::new(),
            file: None,
            file_ct: None,
            title: "FastLogViewer".to_string(),
            search: String::new(),
            regex_on: false,
            case_on: false,
            filter_on: false,
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
            type_deadline: None,
            popup: None,
            goto_open: false,
            goto_text: String::new(),
            goto_focus: false,
            search_focus: false,
            chrome_applied: false,
        };

        app.tag_store.load();
        app.sync_view_tags();

        if let Some(path) = initial_file {
            if Path::new(&path).is_file() {
                app.load_file(&path);
            }
        }
        app
    }

    fn sync_view_tags(&mut self) {
        self.view.set_tags(self.tag_store.tags.clone());
    }

    // ------------------------------------------------------------ file loading

    pub fn load_file(&mut self, path: &str) {
        self.release_current();

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

        let filtered: Vec<Uuid> = self
            .tag_store
            .tags
            .iter()
            .filter(|t| t.filter_on)
            .map(|t| t.id)
            .collect();
        for id in filtered {
            self.start_tag_filter(id);
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
        let Some(file) = self.file.clone() else { return };

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

    fn start_tag_filter(&mut self, id: Uuid) {
        let Some(file) = self.file.clone() else { return };
        let Some(matcher) = self
            .tag_store
            .tags
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.matcher.clone())
        else {
            return;
        };
        if !matcher.is_valid() || matcher.is_empty() {
            return;
        }
        self.stop_tag_filter(id);

        let index = Arc::new(MatchIndex::new(matcher, file.length()));
        self.tag_indexes.insert(id, index.clone());
        let ct = CancelToken::new();
        self.tag_cts.insert(id, ct.clone());
        std::thread::spawn(move || index.build(&file, &ct));
        self.rebuild_union();
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

        let ad_hoc_live = self.filter_on && self.matches.is_some();
        let tag_on = self.tag_union.as_ref().is_some_and(|u| u.is_complete());
        let ad_hoc_complete = self.matches.as_ref().is_some_and(|m| m.is_complete());

        let (mut result, base): (Option<Arc<dyn FilterIndex>>, Base) =
            if ad_hoc_live && tag_on && ad_hoc_complete {
                let matches = self.matches.clone().unwrap();
                let union = self.tag_union.clone().unwrap();
                (
                    Some(Arc::new(IntersectionIndex::new(
                        matches.as_ref(),
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
                // Live, incremental.
                (
                    Some(self.matches.clone().unwrap() as Arc<dyn FilterIndex>),
                    Base::AdHoc,
                )
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
        let Some(file) = self.file.clone() else { return };

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

        let ad_hoc_live =
            self.filter_on && self.matches.as_ref().is_some_and(|m| m.is_complete());
        let tag_on = self.tag_union.as_ref().is_some_and(|u| u.is_complete());

        if ad_hoc_live && tag_on {
            let matches = self.matches.clone().unwrap();
            let union = self.tag_union.clone().unwrap();
            return Some(Arc::new(IntersectionIndex::new(
                matches.as_ref(),
                union.as_ref(),
                file.length(),
            )));
        }
        if tag_on {
            return Some(self.tag_union.clone().unwrap());
        }
        if ad_hoc_live {
            return Some(self.matches.clone().unwrap());
        }
        None
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
            let from = if current < 0 { i32::MAX } else { current as i32 };
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

    // ----------------------------------------------------------------- status

    fn refresh_status(&mut self, ctx: &Context) {
        let Some(file) = self.file.clone() else { return };
        self.view.sync_scroll_bars();

        let size = format!("{} MB", thousands_1dp(file.length() as f64 / 1_048_576.0));
        let mut busy = false;
        let mut left;

        if file.index_complete() {
            let sel = self.view.selected_actual_line();
            left = format!(
                "{} lines   ·   {}{}{}",
                thousands(file.line_count() as u64),
                size,
                if sel >= 0 {
                    format!("   ·   line {}", thousands(sel as u64 + 1))
                } else {
                    String::new()
                },
                if self.view.filter_on() {
                    "   ·   filtered"
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
                "{} lines   ·   {}   ·   indexing {:.0}%",
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
                "clustering… {:.0}%   ·   {} patterns so far",
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
                "{} templates   ·   {} lines",
                thousands(t.template_count() as u64),
                thousands(t.clustered_lines() as u64)
            );
        } else if let Some(zoom) = self.zoom.clone() {
            left = format!(
                "▲ {}   ·   {} lines",
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
            self.count_text = if matches.is_complete() {
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

        // The C# stopped a 100 ms WinForms timer here; the egui equivalent is to stop asking for
        // repaints once nothing is in flight.
        if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // ------------------------------------------------------------------ panels

    fn search_bar(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::CHROME);
        theme::hairline(
            &painter,
            theme::BORDER,
            pos2(rect.left(), rect.bottom() - 1.0),
            pos2(rect.right(), rect.bottom() - 1.0),
        );

        let ctl_h = 28.0;
        let ctl_y = rect.top() + (BAR_H - ctl_h) / 2.0;
        let pad = 12.0;
        let gap = 8.0;

        let mut right = rect.right() - pad;
        let count_rect = Rect::from_min_size(pos2(right - 130.0, ctl_y), vec2(130.0, ctl_h));
        right -= 130.0 + gap;
        let filter_rect = Rect::from_min_size(pos2(right - 60.0, ctl_y), vec2(60.0, ctl_h));
        right -= 60.0 + 6.0;
        let case_rect = Rect::from_min_size(pos2(right - 38.0, ctl_y), vec2(38.0, ctl_h));
        right -= 38.0 + 6.0;
        let regex_rect = Rect::from_min_size(pos2(right - 38.0, ctl_y), vec2(38.0, ctl_h));
        right -= 38.0 + gap;
        let field_w = (right - (rect.left() + pad)).max(120.0);
        let field_rect = Rect::from_min_size(pos2(rect.left() + pad, ctl_y), vec2(field_w, ctl_h));

        theme::text_in(
            &painter,
            count_rect,
            Align2::RIGHT_CENTER,
            &self.count_text,
            theme::ui_font(8.5),
            if self.count_error {
                theme::ERROR
            } else {
                theme::TEXT_DIM
            },
        );

        if toggle_button(
            ui,
            filter_rect,
            "Filter",
            theme::ui_font(8.5),
            &mut self.filter_on,
            "Show only matching lines  (Ctrl+H)",
        ) {
            self.recompute_active_filter();
        }

        let mut case_on = self.case_on;
        if toggle_button(
            ui,
            case_rect,
            "Aa",
            theme::mono(9.0),
            &mut case_on,
            "Match case",
        ) {
            self.case_on = case_on;
            self.start_match_scan();
        }

        let mut regex_on = self.regex_on;
        if toggle_button(
            ui,
            regex_rect,
            ".*",
            theme::mono(9.0),
            &mut regex_on,
            "Treat the pattern as a regular expression",
        ) {
            self.regex_on = regex_on;
            self.start_match_scan();
        }

        let edit_id = ui.id().with("search_edit");
        let focused = ui.ctx().memory(|m| m.has_focus(edit_id));
        let inner = field_frame(ui, field_rect, focused);
        let edit_rect = Rect::from_center_size(inner.center(), vec2(inner.width(), 20.0));

        let response = ui.put(
            edit_rect,
            TextEdit::singleline(&mut self.search)
                .id(edit_id)
                .frame(false)
                .hint_text("Search…")
                .font(theme::mono(10.0))
                .text_color(theme::TEXT),
        );

        if response.changed() {
            // Debounce typing, same 250 ms the C# `_typeTimer` used.
            self.type_deadline = Some(Instant::now() + Duration::from_millis(250));
        }
        if self.search_focus {
            self.search_focus = false;
            response.request_focus();
            ui.ctx().memory_mut(|m| m.request_focus(edit_id));
        }

        // Enter / Escape inside the search box, the C# `Search_KeyDown`.
        if focused {
            let (enter, shift, escape) = ui.ctx().input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Enter)
                        || i.consume_key(Modifiers::SHIFT, Key::Enter),
                    i.modifiers.shift,
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if enter {
                if self.type_deadline.is_some() {
                    self.start_match_scan();
                }
                self.jump_match(if shift { -1 } else { 1 });
            } else if escape {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.start_match_scan();
                    ui.ctx().memory_mut(|m| m.surrender_focus(edit_id));
                } else {
                    self.step_out_one_zoom_level();
                }
            }
        }
    }

    fn tag_row(&mut self, ui: &mut Ui) {
        let font = theme::ui_font(8.5);
        let tags = self.tag_store.tags.clone();
        let mut actions: Vec<(Uuid, ChipAction)> = Vec::new();
        let mut add_clicked = false;

        eframe::egui::ScrollArea::horizontal()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    for tag in &tags {
                        let width = tag_ui::chip_width(ui.ctx(), tag, &font);
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(width, CHIP_HEIGHT),
                            Sense::hover(),
                        );
                        let action = tag_ui::tag_chip(ui, tag, rect, font.clone());
                        if action != ChipAction::None {
                            actions.push((tag.id, action));
                        }
                        ui.add_space(CHIP_MARGIN * 2.0);
                    }
                    // The + chip always stays last.
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    add_clicked = tag_ui::add_tag_chip(ui, rect, theme::ui_font(11.0));
                });
            });

        for (id, action) in actions {
            self.apply_chip_action(id, action);
        }
        if add_clicked {
            self.popup = Some(TagEditPopup::new_tag(
                self.tag_store.tags.len() % TAG_PALETTE.len(),
            ));
        }
    }

    fn apply_chip_action(&mut self, id: Uuid, action: ChipAction) {
        let Some(index) = self.tag_store.tags.iter().position(|t| t.id == id) else {
            return;
        };

        match action {
            ChipAction::None => {}
            ChipAction::ToggleHighlight => {
                self.tag_store.tags[index].highlight_on = !self.tag_store.tags[index].highlight_on;
                self.tag_store.save();
                self.sync_view_tags();
            }
            ChipAction::ToggleFilter => {
                let on = !self.tag_store.tags[index].filter_on;
                self.tag_store.tags[index].filter_on = on;
                self.tag_store.save();
                if on {
                    self.start_tag_filter(id);
                } else {
                    self.stop_tag_filter(id);
                    self.rebuild_union();
                }
                self.sync_view_tags();
            }
            ChipAction::Remove => {
                self.stop_tag_filter(id);
                self.rebuild_union();
                self.tag_store.remove(index);
                self.sync_view_tags();
            }
            ChipAction::Edit => {
                self.popup = Some(TagEditPopup::edit(&self.tag_store.tags[index]));
            }
        }
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
                let id = self.tag_store.tags[index].id;
                if popup.filter {
                    self.start_tag_filter(id);
                }
            }
            Some(id) => {
                let Some(index) = self.tag_store.tags.iter().position(|t| t.id == id) else {
                    return;
                };
                let was_filter_on = self.tag_store.tags[index].filter_on;
                {
                    let tag = &mut self.tag_store.tags[index];
                    tag.label = popup.result_label();
                    tag.pattern = popup.pattern.clone();
                    tag.ignore_case = popup.ignore_case;
                    tag.highlight_on = popup.highlight;
                    tag.filter_on = popup.filter;
                    tag.color_index = popup.color_index;
                    tag.rebuild_matcher();
                }
                self.tag_store.save();

                if popup.filter {
                    self.start_tag_filter(id); // the pattern may have changed -> restart
                } else if was_filter_on {
                    self.stop_tag_filter(id);
                    self.rebuild_union();
                }
            }
        }
        self.sync_view_tags();
    }

    fn goto_window(&mut self, ctx: &Context) {
        let mut go = false;
        let mut close = false;

        eframe::egui::Window::new("Go to line")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_width(250.0)
            .show(ctx, |ui| {
                let response = ui.add(
                    TextEdit::singleline(&mut self.goto_text).desired_width(f32::INFINITY),
                );
                if self.goto_focus {
                    self.goto_focus = false;
                    response.request_focus();
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() - 80.0);
                    if ui.button("Go").clicked() {
                        go = true;
                    }
                });
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    go = true;
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
            });

        if go {
            if let (Ok(n), Some(file)) = (
                self.goto_text.trim().parse::<i64>(),
                self.file.clone(),
            ) {
                let last = file.line_count() as i64 - 1;
                self.view.go_to_actual((n - 1).clamp(0, last.max(0)));
            }
            close = true;
        }
        if close {
            self.goto_open = false;
            self.goto_text.clear();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        if !self.chrome_applied {
            self.chrome_applied = true;
            apply_window_chrome(frame);
        }
        ctx.send_viewport_cmd(ViewportCommand::Title(self.title.clone()));

        // --- drag and drop ---
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.iter().filter_map(|f| f.path.clone()).next() {
            self.load_file(&path.to_string_lossy());
        }

        // --- debounced typing ---
        if let Some(deadline) = self.type_deadline {
            if Instant::now() >= deadline {
                self.start_match_scan();
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }

        // --- global shortcuts, the C# ProcessCmdKey ---
        let modal_open = self.popup.is_some() || self.goto_open;
        if !modal_open {
            // `Modifiers::COMMAND`, not `CTRL`: on Windows a real ctrl press sets both `ctrl` and
            // `command`, and egui's matcher rejects a `CTRL` pattern against it.
            let (open, find, filter, goto, patterns, next, prev) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::O),
                    i.consume_key(Modifiers::COMMAND, Key::F),
                    i.consume_key(Modifiers::COMMAND, Key::H),
                    i.consume_key(Modifiers::COMMAND, Key::G),
                    i.consume_key(Modifiers::COMMAND, Key::M),
                    i.consume_key(Modifiers::NONE, Key::F3),
                    i.consume_key(Modifiers::SHIFT, Key::F3),
                )
            });
            if open {
                self.open_dialog();
            }
            if find {
                self.search_focus = true;
            }
            if filter {
                self.filter_on = !self.filter_on;
                self.recompute_active_filter();
            }
            if goto && self.file.is_some() {
                self.goto_open = true;
                self.goto_focus = true;
            }
            if patterns {
                self.toggle_semantic_zoom();
            }
            if next {
                self.jump_match(1);
            }
            if prev {
                self.jump_match(-1);
            }
        }

        // --- keys the log surface owns, when no editor has focus ---
        let editor_focused = ctx.memory(|m| m.focused().is_some());
        if !editor_focused && !modal_open {
            let events = self.view.handle_keys(ctx);
            if events.template_activated {
                self.zoom_into_selected_template();
            }
            if events.escape_pressed {
                self.step_out_one_zoom_level();
            }
        }

        // --- panels ---
        TopBottomPanel::top("search_bar")
            .exact_height(BAR_H)
            .frame(Frame::none().fill(theme::CHROME))
            .show(ctx, |ui| self.search_bar(ui));

        TopBottomPanel::top("tag_row")
            .exact_height(TAG_ROW_H)
            .frame(Frame::none().fill(theme::BG).inner_margin(
                eframe::egui::Margin::symmetric(0.0, 4.0),
            ))
            .show(ctx, |ui| self.tag_row(ui));

        TopBottomPanel::bottom("status")
            .exact_height(STATUS_H)
            .frame(Frame::none())
            .show(ctx, |ui| {
                status_strip(
                    ui,
                    ui.max_rect(),
                    &self.status_left,
                    HINTS,
                    theme::TEXT_DIM,
                    theme::ui_font(8.5),
                );
            });

        let events = CentralPanel::default()
            .frame(Frame::none().fill(theme::BG))
            .show(ctx, |ui| self.view.ui(ui))
            .inner;

        if events.template_activated {
            self.zoom_into_selected_template();
        }
        if events.escape_pressed {
            self.step_out_one_zoom_level();
        }

        // --- dialogs ---
        if let Some(mut popup) = self.popup.take() {
            match popup.show(ctx) {
                PopupOutcome::Open => self.popup = Some(popup),
                PopupOutcome::Accepted => self.commit_popup(popup),
                PopupOutcome::Cancelled => {}
            }
        }
        if self.goto_open {
            self.goto_window(ctx);
        }

        self.refresh_status(ctx);
    }
}

fn apply_window_chrome(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = frame.window_handle() {
        if let RawWindowHandle::Win32(win32) = handle.as_raw() {
            win::apply_chrome(win32.hwnd.get() as u64);
        }
    }
}
