// templates.rs — semantic zoom: Drain-style clustering of log lines into templates.
//
// The pass runs on demand (Ctrl+M), on a background worker, over either the whole file or only the
// lines the active filter admits. It never allocates a string per line: lines are copied into a
// reused byte buffer, tokenized in place, and compared to templates byte-wise. Strings appear only
// when a new template is born — a couple hundred times, not a couple million.
//
// See docs/superpowers/specs/2026-08-07-semantic-zoom-log-templating-design.md

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::cancel::CancelToken;
use crate::filter::FilterIndex;
use crate::line_numbers::LineNumberList;
use crate::log_file::LogFile;
use crate::MAX_LINE_BYTES;

/// Which zoom level the view is rendering: individual lines, or clustered patterns.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LogViewMode {
    #[default]
    Lines,
    Templates,
}

// ------------------------------------------------------------------ TokenBuffer

/// Reusable, allocation-free tokenization of one line's bytes. Boundaries fall at whitespace, at
/// digit/alpha/other class transitions, and around every punctuation byte — which is what makes
/// "/api/<*>" and "<*>ms" fall out instead of the coarser whitespace-only tokens plain Drain
/// would produce.
pub struct TokenBuffer {
    pub offset: [usize; TokenBuffer::MAX_TOKENS],
    pub length: [usize; TokenBuffer::MAX_TOKENS],
    pub space_before: [bool; TokenBuffer::MAX_TOKENS],
    pub count: usize,
    pub truncated: bool,
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum Class {
    Space,
    Digit,
    Alpha,
    Other,
}

fn class_of(c: u8) -> Class {
    if c == b' ' || c == b'\t' || c == 13 || c == 10 {
        return Class::Space;
    }
    if c.is_ascii_digit() {
        return Class::Digit;
    }
    // >= 0x80 counts as alpha so a UTF-8 multi-byte sequence stays inside one token.
    if c.is_ascii_alphabetic() || c >= 0x80 {
        return Class::Alpha;
    }
    Class::Other
}

impl Default for TokenBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenBuffer {
    pub const MAX_TOKENS: usize = 64;

    pub fn new() -> Self {
        TokenBuffer {
            offset: [0; Self::MAX_TOKENS],
            length: [0; Self::MAX_TOKENS],
            space_before: [false; Self::MAX_TOKENS],
            count: 0,
            truncated: false,
        }
    }

    pub fn tokenize(&mut self, b: &[u8], start: usize, end: usize) {
        self.count = 0;
        self.truncated = false;
        if start > end || end > b.len() {
            return;
        }

        let mut pending_space = false;
        let mut i = start;
        while i < end {
            let cls = class_of(b[i]);
            if cls == Class::Space {
                pending_space = true;
                i += 1;
                continue;
            }

            if self.count == Self::MAX_TOKENS {
                self.truncated = true;
                return;
            }

            let token_start = i;
            if cls == Class::Other {
                i += 1; // punctuation: always one byte
            } else {
                while i < end && class_of(b[i]) == cls {
                    i += 1;
                }
            }

            self.offset[self.count] = token_start;
            self.length[self.count] = i - token_start;
            self.space_before[self.count] = pending_space;
            self.count += 1;
            pending_space = false;
        }
    }

    /// The raw bytes of token `i` within the line buffer it was tokenized from.
    pub fn token<'a>(&self, b: &'a [u8], i: usize) -> &'a [u8] {
        &b[self.offset[i]..self.offset[i] + self.length[i]]
    }
}

// ------------------------------------------------------------------ PrefixShape

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PrefixKind {
    None = 0,
    Iso = 1,
    Syslog = 2,
    Bracket = 3,
    BareTime = 4,
}

impl PrefixKind {
    fn from_u8(v: u8) -> PrefixKind {
        match v {
            1 => PrefixKind::Iso,
            2 => PrefixKind::Syslog,
            3 => PrefixKind::Bracket,
            4 => PrefixKind::BareTime,
            _ => PrefixKind::None,
        }
    }
}

/// A detected leading timestamp (plus optional level token) that is stripped before clustering.
/// Without this, every line's first token differs, the Drain tree's second level discriminates
/// nothing, and everything collapses into one bucket.
///
/// The timestamp span is measured separately from the full prefix so the deferred time-window diff
/// feature can reuse it; nothing in this feature consumes `measure_timestamp`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PrefixShape {
    pub kind: PrefixKind,
}

const LEVELS: [&[u8]; 7] = [
    b"TRACE", b"DEBUG", b"INFO", b"WARNING", b"WARN", b"ERROR", b"FATAL",
];

const MONTHS: [&[u8; 3]; 12] = [
    b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec",
];

fn digit(b: &[u8], i: usize, len: usize) -> bool {
    i < len && b[i].is_ascii_digit()
}

fn is(b: &[u8], i: usize, len: usize, c: u8) -> bool {
    i < len && b[i] == c
}

/// Count of consecutive digits at `i`, or `None` if fewer than `min`.
fn digits(b: &[u8], i: usize, len: usize, min: usize, max: usize) -> Option<usize> {
    let mut n = 0;
    while n < max && digit(b, i + n, len) {
        n += 1;
    }
    if n >= min {
        Some(n)
    } else {
        None
    }
}

/// hh:mm:ss with optional .fff or ,fff. Returns the end index.
fn time(b: &[u8], mut i: usize, len: usize) -> Option<usize> {
    if digits(b, i, len, 2, 2)? != 2 {
        return None;
    }
    i += 2;
    if !is(b, i, len, b':') {
        return None;
    }
    i += 1;
    if digits(b, i, len, 2, 2)? != 2 {
        return None;
    }
    i += 2;
    if !is(b, i, len, b':') {
        return None;
    }
    i += 1;
    if digits(b, i, len, 2, 2)? != 2 {
        return None;
    }
    i += 2;
    if is(b, i, len, b'.') || is(b, i, len, b',') {
        if let Some(f) = digits(b, i + 1, len, 1, 9) {
            i += 1 + f;
        }
    }
    Some(i)
}

/// yyyy-MM-dd. Returns the end index.
fn date(b: &[u8], mut i: usize, len: usize) -> Option<usize> {
    if digits(b, i, len, 4, 4)? != 4 {
        return None;
    }
    i += 4;
    if !is(b, i, len, b'-') {
        return None;
    }
    i += 1;
    if digits(b, i, len, 2, 2)? != 2 {
        return None;
    }
    i += 2;
    if !is(b, i, len, b'-') {
        return None;
    }
    i += 1;
    if digits(b, i, len, 2, 2)? != 2 {
        return None;
    }
    Some(i + 2)
}

fn iso(b: &[u8], i: usize, len: usize) -> Option<usize> {
    let d = date(b, i, len)?;
    if !is(b, d, len, b'T') && !is(b, d, len, b' ') {
        return None;
    }

    let t = time(b, d + 1, len)?;

    if is(b, t, len, b'Z') {
        return Some(t + 1);
    }
    if is(b, t, len, b'+') || is(b, t, len, b'-') {
        let mut j = t + 1;
        if digits(b, j, len, 2, 2) == Some(2) {
            j += 2;
            if is(b, j, len, b':') {
                j += 1;
            }
            if digits(b, j, len, 2, 2) == Some(2) {
                return Some(j + 2);
            }
        }
        return Some(t); // a trailing '-' that isn't an offset
    }
    Some(t)
}

fn syslog(b: &[u8], i: usize, len: usize) -> Option<usize> {
    let mut month = false;
    for name in MONTHS.iter() {
        if month {
            break;
        }
        if i + 3 > len {
            break;
        }
        month = b[i] == name[0] && b[i + 1] == name[1] && b[i + 2] == name[2];
    }
    if !month {
        return None;
    }

    let mut j = i + 3;
    while is(b, j, len, b' ') {
        j += 1;
    }
    let day = digits(b, j, len, 1, 2)?;
    j += day;
    while is(b, j, len, b' ') {
        j += 1;
    }
    time(b, j, len)
}

fn bracket(b: &[u8], i: usize, len: usize) -> Option<usize> {
    if !is(b, i, len, b'[') {
        return None;
    }
    let inner = iso(b, i + 1, len).or_else(|| time(b, i + 1, len))?;
    if is(b, inner, len, b']') {
        Some(inner + 1)
    } else {
        None
    }
}

fn match_kind(kind: PrefixKind, b: &[u8], len: usize) -> Option<usize> {
    match kind {
        PrefixKind::Iso => iso(b, 0, len),
        PrefixKind::Syslog => syslog(b, 0, len),
        PrefixKind::Bracket => bracket(b, 0, len),
        PrefixKind::BareTime => time(b, 0, len),
        PrefixKind::None => None,
    }
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// A bare or bracketed level word, case-insensitive. Returns the end index.
fn match_level(b: &[u8], i: usize, len: usize) -> Option<usize> {
    let bracketed = is(b, i, len, b'[');
    let j = if bracketed { i + 1 } else { i };

    for w in LEVELS.iter() {
        if j + w.len() > len {
            continue;
        }

        let hit = w.iter().enumerate().all(|(c, &want)| {
            let got = b[j + c];
            let upper = if got.is_ascii_lowercase() { got - 32 } else { got };
            upper == want
        });
        if !hit {
            continue;
        }

        let end = j + w.len();
        if bracketed {
            return if is(b, end, len, b']') {
                Some(end + 1)
            } else {
                None
            };
        }
        if end >= len || !is_word_byte(b[end]) {
            return Some(end); // must be a whole word
        }
    }
    None
}

impl PrefixShape {
    pub const NONE: PrefixShape = PrefixShape {
        kind: PrefixKind::None,
    };

    /// Length of just the timestamp portion, or 0 if this line doesn't carry one.
    pub fn measure_timestamp(&self, b: &[u8], len: usize) -> usize {
        if self.kind == PrefixKind::None {
            return 0;
        }
        match_kind(self.kind, b, len).unwrap_or(0)
    }

    /// Bytes to skip before clustering: timestamp, then an optional level token, then the
    /// whitespace that follows. Recomputed per line — timestamps vary in length, so this is never
    /// a fixed offset.
    pub fn prefix_length(&self, b: &[u8], len: usize) -> usize {
        if self.kind == PrefixKind::None {
            return 0;
        }

        // This line doesn't match the sniffed shape — strip nothing.
        let Some(after_ts) = match_kind(self.kind, b, len) else {
            return 0;
        };

        let mut i = after_ts;
        while is(b, i, len, b' ') {
            i += 1;
        }

        let Some(after_level) = match_level(b, i, len) else {
            return i;
        };

        i = after_level;
        while is(b, i, len, b' ') {
            i += 1;
        }
        i
    }

    /// Picks the first shape matched by at least 80% of the sampled lines.
    pub fn sniff(sample_lines: &[Vec<u8>]) -> PrefixShape {
        if sample_lines.is_empty() {
            return PrefixShape::NONE;
        }

        let order = [
            PrefixKind::Iso,
            PrefixKind::Syslog,
            PrefixKind::Bracket,
            PrefixKind::BareTime,
        ];
        let needed = (sample_lines.len() * 8).div_ceil(10); // ceil(80%)

        for kind in order {
            let hits = sample_lines
                .iter()
                .filter(|line| match_kind(kind, line, line.len()).unwrap_or(0) > 0)
                .count();
            if hits >= needed {
                return PrefixShape { kind };
            }
        }
        PrefixShape::NONE
    }
}

// ------------------------------------------------------------------ LogTemplate

/// One clustered pattern. Tokens are stored as raw bytes so matching never allocates; a token
/// flagged as a slot is a variable position.
///
/// Implements `FilterIndex` directly — that is what makes "click a template, show its lines" a
/// single `set_active_filter` call against machinery that already works, with no adapter and no
/// per-line template-id array.
///
/// Threading: built on the clustering worker, read concurrently by the UI. The token *bytes* are
/// immutable for the template's whole life; becoming a slot only flips an `AtomicBool`, so a reader
/// sees either the literal or the slot — both render correctly. The cached render is invalidated
/// whenever a slot appears.
pub struct LogTemplate {
    pub id: usize,
    tokens: Vec<Vec<u8>>,
    is_slot: Vec<AtomicBool>,
    pub space_before: Vec<bool>,
    pub truncated: bool,

    lines: LineNumberList,
    rendered: Mutex<Option<String>>,
    slot_count: AtomicUsize,
}

impl LogTemplate {
    pub fn new(
        id: usize,
        tokens: Vec<Vec<u8>>,
        space_before: Vec<bool>,
        truncated: bool,
        capacity_hint: i64,
    ) -> Self {
        let is_slot = (0..tokens.len()).map(|_| AtomicBool::new(false)).collect();
        LogTemplate {
            id,
            tokens,
            is_slot,
            space_before,
            truncated,
            lines: LineNumberList::new(capacity_hint),
            rendered: Mutex::new(None),
            slot_count: AtomicUsize::new(0),
        }
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// The literal bytes at position `i`, or `None` if that position is a variable slot.
    pub fn token(&self, i: usize) -> Option<&[u8]> {
        if self.is_slot[i].load(Ordering::Acquire) {
            None
        } else {
            Some(&self.tokens[i])
        }
    }

    pub fn slot_count(&self) -> usize {
        self.slot_count.load(Ordering::Acquire)
    }

    pub fn add_line(&self, line: i32) {
        self.lines.add(line);
    }

    pub fn make_slot(&self, index: usize) {
        if self.is_slot[index].swap(true, Ordering::Release) {
            return; // already a slot; don't double-count
        }
        self.slot_count.fetch_add(1, Ordering::AcqRel);
        *self.rendered.lock().unwrap() = None;
    }

    /// Token positions that are variable slots, in order. Slot N in a query is
    /// `slot_token_indexes()[N - 1]`.
    pub fn slot_token_indexes(&self) -> Vec<usize> {
        (0..self.tokens.len())
            .filter(|&i| self.token(i).is_none())
            .collect()
    }

    pub fn render(&self) -> String {
        let mut cache = self.rendered.lock().unwrap();
        if let Some(cached) = cache.as_ref() {
            return cached.clone();
        }

        let text = if self.tokens.is_empty() {
            "<unclustered>".to_string()
        } else {
            let mut s = String::new();
            for i in 0..self.tokens.len() {
                if self.space_before[i] && !s.is_empty() {
                    s.push(' ');
                }
                match self.token(i) {
                    None => s.push_str("<*>"),
                    Some(t) => s.push_str(&String::from_utf8_lossy(t)),
                }
            }
            if self.truncated {
                s.push_str(" …");
            }
            s
        };

        *cache = Some(text.clone());
        text
    }
}

impl FilterIndex for LogTemplate {
    fn count(&self) -> usize {
        self.lines.count()
    }
    fn get(&self, i: usize) -> i32 {
        self.lines.get(i)
    }
    fn index_at_or_after(&self, line: i32) -> usize {
        self.lines.index_at_or_after(line)
    }
}

// ----------------------------------------------------------------- TemplateTree

const WILDCARD_KEY: u32 = 0; // templates whose token 0 became a slot

struct TreeState {
    /// token count -> first-token hash -> candidate template indices
    by_count: Vec<Option<HashMap<u32, Vec<usize>>>>,
    overflow: Option<usize>,
}

/// Fixed-depth Drain tree: token count -> first-token hash -> candidate templates. Existing slots
/// match anything at zero weight, so a template loosens as it absorbs variety. Beyond
/// `MAX_TEMPLATES` nothing new is created and stragglers land in one overflow bucket — the guard
/// against a file of entirely unique lines.
///
/// The bucket map is touched only by the clustering worker, so its `Mutex` is uncontended; the
/// published template list sits behind an `RwLock` the UI can snapshot while the build runs.
pub struct TemplateTree {
    capacity_hint: i64,
    state: Mutex<TreeState>,
    templates: RwLock<Vec<Arc<LogTemplate>>>,
}

fn hash_token(bytes: &[u8]) -> u32 {
    let mut h: u32 = 2166136261; // FNV-1a
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    if h == WILDCARD_KEY {
        1
    } else {
        h // never collide with the wildcard bucket
    }
}

impl Default for TemplateTree {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl TemplateTree {
    pub const MAX_TEMPLATES: usize = 5000;
    pub const THRESHOLD: f64 = 0.5;

    pub fn new(capacity_hint: i64) -> Self {
        TemplateTree {
            capacity_hint,
            state: Mutex::new(TreeState {
                by_count: (0..=TokenBuffer::MAX_TOKENS).map(|_| None).collect(),
                overflow: None,
            }),
            templates: RwLock::new(Vec::new()),
        }
    }

    pub fn count(&self) -> usize {
        self.templates.read().unwrap().len()
    }

    pub fn snapshot(&self) -> Vec<Arc<LogTemplate>> {
        self.templates.read().unwrap().clone()
    }

    pub fn ingest(&self, tb: &TokenBuffer, b: &[u8], line: i32) -> Arc<LogTemplate> {
        let mut state = self.state.lock().unwrap();

        let n = tb.count;
        if n == 0 {
            return self.add_to_overflow(&mut state, line);
        }
        if state.by_count[n].is_none() {
            state.by_count[n] = Some(HashMap::new());
        }

        let key = hash_token(tb.token(b, 0));

        let best = {
            let templates = self.templates.read().unwrap();
            let level2 = state.by_count[n].as_ref().unwrap();
            let mut best: Option<usize> = None;
            let mut best_sim = Self::THRESHOLD;
            probe(level2, key, tb, b, &templates, &mut best, &mut best_sim);
            probe(
                level2,
                WILDCARD_KEY,
                tb,
                b,
                &templates,
                &mut best,
                &mut best_sim,
            );
            best
        };

        if let Some(best_index) = best {
            let template = self.templates.read().unwrap()[best_index].clone();
            let mut token0_became_slot = false;

            for i in 0..n {
                let Some(tok) = template.token(i) else { continue };
                if tok == tb.token(b, i) {
                    continue;
                }
                template.make_slot(i);
                if i == 0 {
                    token0_became_slot = true; // no longer findable by first-token hash
                }
            }
            if token0_became_slot {
                rebucket(state.by_count[n].as_mut().unwrap(), best_index);
            }

            template.add_line(line);
            return template;
        }

        if self.templates.read().unwrap().len() >= Self::MAX_TEMPLATES {
            return self.add_to_overflow(&mut state, line);
        }

        let tokens: Vec<Vec<u8>> = (0..n).map(|i| tb.token(b, i).to_vec()).collect();
        let spaces: Vec<bool> = (0..n).map(|i| tb.space_before[i]).collect();

        let mut templates = self.templates.write().unwrap();
        let created = Arc::new(LogTemplate::new(
            templates.len(),
            tokens,
            spaces,
            tb.truncated,
            self.capacity_hint,
        ));
        created.add_line(line);

        state
            .by_count[n]
            .as_mut()
            .unwrap()
            .entry(key)
            .or_default()
            .push(templates.len());
        templates.push(created.clone());
        created
    }

    fn add_to_overflow(&self, state: &mut TreeState, line: i32) -> Arc<LogTemplate> {
        if state.overflow.is_none() {
            let mut templates = self.templates.write().unwrap();
            // The overflow bucket is appended like any other template, so it never displaces a
            // real one; its empty token list is what renders as "<unclustered>".
            let overflow = Arc::new(LogTemplate::new(
                templates.len(),
                Vec::new(),
                Vec::new(),
                false,
                self.capacity_hint,
            ));
            state.overflow = Some(templates.len());
            templates.push(overflow);
        }
        let overflow = self.templates.read().unwrap()[state.overflow.unwrap()].clone();
        overflow.add_line(line);
        overflow
    }
}

#[allow(clippy::too_many_arguments)]
fn probe(
    level2: &HashMap<u32, Vec<usize>>,
    key: u32,
    tb: &TokenBuffer,
    b: &[u8],
    templates: &[Arc<LogTemplate>],
    best: &mut Option<usize>,
    best_sim: &mut f64,
) {
    let Some(bucket) = level2.get(&key) else { return };

    for &index in bucket {
        let candidate = &templates[index];
        let mut comparable = 0usize;
        let mut hits = 0usize;
        for i in 0..tb.count {
            let Some(tok) = candidate.token(i) else {
                continue; // slot: zero weight, matches anything
            };
            comparable += 1;
            if tok == tb.token(b, i) {
                hits += 1;
            }
        }
        let sim = if comparable == 0 {
            1.0
        } else {
            hits as f64 / comparable as f64
        };
        if sim >= *best_sim {
            *best_sim = sim;
            *best = Some(index);
        }
    }
}

fn rebucket(level2: &mut HashMap<u32, Vec<usize>>, index: usize) {
    for bucket in level2.values_mut() {
        if let Some(at) = bucket.iter().position(|&i| i == index) {
            bucket.remove(at);
            break;
        }
    }
    level2.entry(WILDCARD_KEY).or_default().push(index);
}

// ---------------------------------------------------------------- TemplateIndex

const SAMPLE_LINES: usize = 1000;
const CANCEL_STRIDE: usize = 0xFFF;

/// Clusters lines into templates on a background worker. Mirrors `MatchIndex`: waits for the line
/// index, publishes progress, checks cancellation on a stride, sets complete last. A `None` scope
/// clusters the whole file; otherwise only the lines the active filter admits.
pub struct TemplateIndex {
    tree: TemplateTree,
    complete: AtomicBool,
    progress: AtomicUsize,        // lines visited so far
    clustered_lines: AtomicUsize, // lines that actually entered a template
    scope_count: AtomicUsize,     // lines this pass will visit in total
    prefix: AtomicU8,
}

impl Default for TemplateIndex {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl TemplateIndex {
    pub fn new(capacity_hint: i64) -> Self {
        TemplateIndex {
            tree: TemplateTree::new(capacity_hint),
            complete: AtomicBool::new(false),
            progress: AtomicUsize::new(0),
            clustered_lines: AtomicUsize::new(0),
            scope_count: AtomicUsize::new(0),
            prefix: AtomicU8::new(PrefixKind::None as u8),
        }
    }

    /// A template is rare below this count. At 4.1M lines the threshold is 41, which flags a 3x
    /// anomaly and leaves a 412x retry pattern alone.
    pub fn rare_threshold_for(clustered_lines: usize) -> usize {
        (clustered_lines / 100_000).max(5)
    }

    pub fn rare_threshold(&self) -> usize {
        Self::rare_threshold_for(self.clustered_lines())
    }

    pub fn is_rare(&self, t: &LogTemplate) -> bool {
        t.count() <= self.rare_threshold()
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }

    pub fn clustered_lines(&self) -> usize {
        self.clustered_lines.load(Ordering::Relaxed)
    }

    pub fn scope_count(&self) -> usize {
        self.scope_count.load(Ordering::Relaxed)
    }

    pub fn prefix(&self) -> PrefixShape {
        PrefixShape {
            kind: PrefixKind::from_u8(self.prefix.load(Ordering::Acquire)),
        }
    }

    /// Templates discovered so far. Cheap — no snapshot, no sort.
    pub fn template_count(&self) -> usize {
        self.tree.count()
    }

    /// Templates by descending count, ties broken by id so the order is stable while the build is
    /// still running. Safe to call mid-build.
    pub fn snapshot_sorted(&self) -> Vec<Arc<LogTemplate>> {
        let mut all = self.tree.snapshot();
        all.sort_by(|a, b| b.count().cmp(&a.count()).then(a.id.cmp(&b.id)));
        all
    }

    pub fn build(&self, f: &LogFile, scope: Option<&dyn FilterIndex>, ct: &CancelToken) {
        // The line index must be finished before line numbers mean anything.
        while !f.index_complete() {
            if ct.is_cancelled() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let n = match scope {
            Some(s) => s.count(),
            None => f.line_count(),
        };
        self.scope_count.store(n, Ordering::Relaxed);

        let mut buf = vec![0u8; MAX_LINE_BYTES];
        let prefix = sniff_prefix(f, scope, n, &mut buf);
        self.prefix.store(prefix.kind as u8, Ordering::Release);

        let mut tb = TokenBuffer::new();
        for i in 0..n {
            let line = match scope {
                Some(s) => s.get(i) as usize,
                None => i,
            };
            let len = f.copy_line(line, &mut buf);
            if len > 0 {
                let start = prefix.prefix_length(&buf, len);
                tb.tokenize(&buf, start, len);
                self.tree.ingest(&tb, &buf, line as i32);
                self.clustered_lines.fetch_add(1, Ordering::Relaxed);
            }

            if (i & CANCEL_STRIDE) == 0 {
                self.progress.store(i, Ordering::Relaxed);
                if ct.is_cancelled() {
                    return;
                }
            }
        }

        self.progress.store(n, Ordering::Relaxed);
        self.complete.store(true, Ordering::Release);
    }
}

fn sniff_prefix(
    f: &LogFile,
    scope: Option<&dyn FilterIndex>,
    n: usize,
    buf: &mut [u8],
) -> PrefixShape {
    let mut sample: Vec<Vec<u8>> = Vec::new();
    let take = SAMPLE_LINES.min(n);
    for i in 0..take {
        let line = match scope {
            Some(s) => s.get(i) as usize,
            None => i,
        };
        let len = f.copy_line(line, buf);
        if len == 0 {
            continue;
        }
        sample.push(buf[..len].to_vec());
    }
    PrefixShape::sniff(&sample)
}
