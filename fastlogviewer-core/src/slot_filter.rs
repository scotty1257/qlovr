use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::cancel::CancelToken;
use crate::filter::FilterIndex;
use crate::line_numbers::LineNumberList;
use crate::log_file::LogFile;
use crate::slot_query::SlotQuery;
use crate::templates::{LogTemplate, PrefixShape, TokenBuffer};
use crate::MAX_LINE_BYTES;

const CANCEL_STRIDE: usize = 0xFFF;

/// The lines of one template whose slots satisfy a `SlotQuery`. Slot values are re-extracted per
/// line rather than stored, so memory stays flat no matter how many lines the template covers. Same
/// worker/progress/cancellation shape as `MatchIndex`.
pub struct SlotFilterIndex {
    lines: LineNumberList,
    template: Arc<LogTemplate>,
    query: Arc<SlotQuery>,
    prefix: PrefixShape,

    complete: AtomicBool,
    progress: AtomicUsize,
}

impl SlotFilterIndex {
    pub fn new(
        template: Arc<LogTemplate>,
        query: Arc<SlotQuery>,
        prefix: PrefixShape,
        capacity_hint: u64,
    ) -> Self {
        SlotFilterIndex {
            lines: LineNumberList::new(capacity_hint as i64),
            template,
            query,
            prefix,
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
        if self.query.error.is_some() {
            self.complete.store(true, Ordering::Release);
            return;
        }

        let slot_positions = self.template.slot_token_indexes();
        let mut values: Vec<Option<String>> = vec![None; slot_positions.len()];
        let mut buf = vec![0u8; MAX_LINE_BYTES];
        let mut tb = TokenBuffer::new();

        let n = self.template.count();
        for i in 0..n {
            let line = self.template.get(i) as usize;
            let len = f.copy_line(line, &mut buf);
            if len > 0 {
                let start = self.prefix.prefix_length(&buf, len);
                tb.tokenize(&buf, start, len);

                for (s, &pos) in slot_positions.iter().enumerate() {
                    values[s] = if pos < tb.count {
                        Some(String::from_utf8_lossy(tb.token(&buf, pos)).into_owned())
                    } else {
                        None // short line: this slot simply can't match
                    };
                }

                if self.query.evaluate(&values) {
                    self.lines.add(line as i32);
                }
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

impl FilterIndex for SlotFilterIndex {
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
