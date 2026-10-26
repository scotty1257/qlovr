use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::cancel::CancelToken;
use crate::line_numbers::LineNumberList;
use crate::match_index::MatchIndex;

/// A sorted set of file line numbers that the view can display instead of the whole file.
///
/// `Send + Sync` is part of the contract: every implementation is built on a worker and read by the
/// UI thread through an `Arc`, which is exactly what makes "click a template, show its lines" a
/// single call against machinery that already works.
pub trait FilterIndex: Send + Sync {
    fn count(&self) -> usize;
    fn get(&self, i: usize) -> i32;
    /// First entry index whose line is >= `line`, or `count()` if none.
    fn index_at_or_after(&self, line: i32) -> usize;
}

/// Merges the sorted, complete line-number lists of several `MatchIndex` objects into one sorted,
/// deduplicated list (OR across tags). Waits for every source to finish building, same wait pattern
/// `MatchIndex::build` uses for the line index.
pub struct UnionFilterIndex {
    lines: LineNumberList,
    sources: Vec<Arc<MatchIndex>>,
    complete: AtomicBool,
}

impl UnionFilterIndex {
    pub fn new(sources: Vec<Arc<MatchIndex>>, file_length: u64) -> Self {
        UnionFilterIndex {
            lines: LineNumberList::new(file_length as i64),
            sources,
            complete: AtomicBool::new(false),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn build(&self, ct: &CancelToken) {
        if self.sources.is_empty() {
            self.complete.store(true, Ordering::Release);
            return;
        }

        for m in &self.sources {
            while !m.is_complete() {
                if ct.is_cancelled() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let mut pos = vec![0usize; self.sources.len()];
        let mut last: i32 = -1;
        loop {
            let mut best_src: Option<usize> = None;
            let mut best_line = i32::MAX;
            for (s, src) in self.sources.iter().enumerate() {
                if pos[s] >= src.count() {
                    continue;
                }
                let v = src.get(pos[s]);
                if v < best_line {
                    best_line = v;
                    best_src = Some(s);
                }
            }
            let Some(s) = best_src else { break };
            pos[s] += 1;
            if best_line != last {
                self.lines.add(best_line);
                last = best_line;
            }
            if ct.is_cancelled() {
                return;
            }
        }
        self.complete.store(true, Ordering::Release);
    }
}

impl FilterIndex for UnionFilterIndex {
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

/// Line numbers present in both sources (AND). Both sources must already be complete — callers only
/// construct this once `a` and `b` are known to have finished building.
pub struct IntersectionIndex {
    lines: LineNumberList,
}

impl IntersectionIndex {
    pub fn new(a: &dyn FilterIndex, b: &dyn FilterIndex, file_length: u64) -> Self {
        let lines = LineNumberList::new(file_length as i64);
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.count() && j < b.count() {
            let (va, vb) = (a.get(i), b.get(j));
            if va == vb {
                lines.add(va);
                i += 1;
                j += 1;
            } else if va < vb {
                i += 1;
            } else {
                j += 1;
            }
        }
        IntersectionIndex { lines }
    }
}

impl FilterIndex for IntersectionIndex {
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

/// Every line *not* in `source` (NOT), for "filter away from" a search. Nothing is copied: each
/// lookup is a binary search over the source, so the complement of a million matches costs nothing
/// to build. The source must be complete — an unfinished scan would let not-yet-found matches
/// through.
pub struct ComplementIndex {
    source: Arc<dyn FilterIndex>,
    total_lines: usize,
}

impl ComplementIndex {
    pub fn new(source: Arc<dyn FilterIndex>, total_lines: usize) -> Self {
        ComplementIndex {
            source,
            total_lines,
        }
    }
}

impl FilterIndex for ComplementIndex {
    fn count(&self) -> usize {
        self.total_lines.saturating_sub(self.source.count())
    }

    /// The i-th kept line is `i + j`, where `j` is how many source lines come before it. Because
    /// source lines are sorted and distinct, `source[j] - j` never decreases, so `j` is the first
    /// position where it exceeds `i`.
    fn get(&self, i: usize) -> i32 {
        let (mut lo, mut hi) = (0usize, self.source.count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if (self.source.get(mid) as i64 - mid as i64) <= i as i64 {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (i + lo) as i32
    }

    /// Kept lines before `line` = all lines before it minus the source lines before it.
    fn index_at_or_after(&self, line: i32) -> usize {
        let line = line.clamp(0, self.total_lines as i32);
        line as usize - self.source.index_at_or_after(line)
    }
}
