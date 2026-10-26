use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::cancel::CancelToken;
use crate::filter::FilterIndex;
use crate::line_numbers::LineNumberList;
use crate::log_file::LogFile;
use crate::matcher::Matcher;
use crate::MAX_LINE_BYTES;

/// Line numbers matching the current pattern. Built on a worker, read lock-free by the UI.
pub struct MatchIndex {
    lines: LineNumberList,
    pub matcher: Arc<Matcher>,
    complete: AtomicBool,
    progress: AtomicUsize, // lines scanned so far
}

impl MatchIndex {
    pub fn new(matcher: Arc<Matcher>, file_length: u64) -> Self {
        MatchIndex {
            lines: LineNumberList::new(file_length as i64),
            matcher,
            complete: AtomicBool::new(false),
            progress: AtomicUsize::new(0),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }

    pub fn build(&self, f: &LogFile, ct: &CancelToken) {
        if self.matcher.is_empty() || !self.matcher.is_valid() {
            self.complete.store(true, Ordering::Release);
            return;
        }

        // The line index must be finished before we can map offsets -> line numbers.
        while !f.index_complete() {
            if ct.is_cancelled() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let total = f.line_count();

        if let Some(literal) = &self.matcher.literal_bytes {
            // Fast path: scan raw bytes (~1 GB/s), only decode the lines that actually hit.
            let mut pos: i64 = 0;
            loop {
                let hit = f.find(literal, pos);
                if hit < 0 {
                    break;
                }

                let line = f.line_of_offset(hit);
                if self.matcher.is_match(&f.get_line(line, MAX_LINE_BYTES)) {
                    self.lines.add(line as i32);
                }

                self.progress.store(line, Ordering::Relaxed);
                if line + 1 >= total {
                    break;
                }
                pos = f.start_of(line + 1); // one hit per line is enough
                if ct.is_cancelled() {
                    return;
                }
            }
            self.progress.store(total, Ordering::Relaxed);
        } else {
            // Regex / case-insensitive path: decode and test every line.
            for i in 0..total {
                if self.matcher.is_match(&f.get_line(i, MAX_LINE_BYTES)) {
                    self.lines.add(i as i32);
                }
                if (i & 0xFFFF) == 0 {
                    self.progress.store(i, Ordering::Relaxed);
                    if ct.is_cancelled() {
                        return;
                    }
                }
            }
            self.progress.store(total, Ordering::Relaxed);
        }

        self.complete.store(true, Ordering::Release);
    }
}

impl FilterIndex for MatchIndex {
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
