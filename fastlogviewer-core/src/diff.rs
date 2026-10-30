//! What changed in a log between two loads of it, by line.
//!
//! A log almost always changes by growing at the end, now and then by being cut short or rewritten.
//! So instead of a general diff, the two versions are compared from both ends: the common leading
//! bytes and the common trailing bytes are unchanged, and the one block between them is what
//! changed. Appends, truncation and a single edited region come out exact; several scattered edits
//! come out as one block spanning them all. Only the new version needs a line index.
//!
//! A `ChangeSet` gathers those blocks over several reloads, renumbered into the newest version's
//! lines, so the marks keep showing everything since the user last cleared them.

use crate::cancel::CancelToken;
use crate::filter::FilterIndex;
use crate::log_file::LogFile;

/// Compared this many bytes at a time, checking for cancellation in between.
const CHUNK: usize = 16 << 20;

/// New lines `first .. first + added` took the place of `removed` old lines starting at `first`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineDiff {
    pub first: usize,
    pub added: usize,
    pub removed: usize,
}

/// How a line came to differ from the version the marks started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// New, in place of nothing.
    Added,
    /// New, in place of other lines.
    Changed,
}

/// Compares two versions of a log. `new` must have a complete line index. Returns `None` when they
/// are the same or the comparison was cancelled.
pub fn diff_logs(old: &LogFile, new: &LogFile, ct: &CancelToken) -> Option<LineDiff> {
    let (a, b) = (old.bytes(), new.bytes());
    let prefix = common_prefix(a, b, ct)?;
    if prefix == a.len() && prefix == b.len() {
        return None;
    }
    // Lines that end inside the common prefix are the same in both.
    let start = b[..prefix]
        .iter()
        .rposition(|&c| c == b'\n')
        .map_or(0, |i| i + 1);

    let suffix = common_suffix(&a[start..], &b[start..], ct)?;
    let (end_a, end_b) = (a.len() - suffix, b.len() - suffix);
    // The first line of the common tail that is whole in both versions. A tail that begins
    // exactly at a line start in both is whole from there; otherwise its first line is
    // partly in the changed block and only the lines after its '\n' are the same.
    let same_b = if is_line_start(b, end_b) && is_line_start(a, end_a) {
        end_b
    } else {
        b[end_b..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |i| end_b + i + 1)
    };
    let same_a = same_b - end_b + end_a;

    let first = line_at(new, start);
    let added = line_at(new, same_b) - first;
    let removed = count_lines(&a[start..same_a]);
    Some(LineDiff {
        first,
        added,
        removed,
    })
}

fn is_line_start(bytes: &[u8], at: usize) -> bool {
    at == 0 || bytes[at - 1] == b'\n'
}

/// The line starting at byte `at`, or the line count when `at` is the end of the file.
fn line_at(file: &LogFile, at: usize) -> usize {
    if at >= file.length() as usize {
        file.line_count()
    } else {
        file.line_of_offset(at as i64)
    }
}

/// Lines in a run of whole lines, counting an unterminated last one.
fn count_lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|&&c| c == b'\n').count();
    newlines + usize::from(bytes.last().is_some_and(|&c| c != b'\n'))
}

fn common_prefix(a: &[u8], b: &[u8], ct: &CancelToken) -> Option<usize> {
    let len = a.len().min(b.len());
    let mut i = 0;
    while i < len {
        if ct.is_cancelled() {
            return None;
        }
        let end = (i + CHUNK).min(len);
        if a[i..end] != b[i..end] {
            return Some(
                i + a[i..end]
                    .iter()
                    .zip(&b[i..end])
                    .take_while(|(x, y)| x == y)
                    .count(),
            );
        }
        i = end;
    }
    Some(len)
}

fn common_suffix(a: &[u8], b: &[u8], ct: &CancelToken) -> Option<usize> {
    let len = a.len().min(b.len());
    let mut n = 0;
    while n < len {
        if ct.is_cancelled() {
            return None;
        }
        let step = CHUNK.min(len - n);
        let (ta, tb) = (
            &a[a.len() - n - step..a.len() - n],
            &b[b.len() - n - step..b.len() - n],
        );
        if ta != tb {
            return Some(
                n + ta
                    .iter()
                    .rev()
                    .zip(tb.iter().rev())
                    .take_while(|(x, y)| x == y)
                    .count(),
            );
        }
        n += step;
    }
    Some(len)
}

/// One changed block: lines `start .. start + len` of the current version, which took the place
/// of `removed` lines of the version the marks started from. `len == 0` is a pure deletion just
/// before line `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeMark {
    pub start: usize,
    pub len: usize,
    pub removed: usize,
}

/// Every change since the marks were last cleared, in the current version's line numbers.
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    marks: Vec<ChangeMark>,
    /// For each mark, how many changed lines come before it; this is the filter's lookup table.
    before: Vec<usize>,
}

impl ChangeSet {
    pub fn marks(&self) -> &[ChangeMark] {
        &self.marks
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Lines now in the file that are new or changed.
    pub fn changed_lines(&self) -> usize {
        self.marks.iter().map(|m| m.len).sum()
    }

    /// Lines of the original version that are gone or were replaced.
    pub fn removed_lines(&self) -> usize {
        self.marks.iter().map(|m| m.removed).sum()
    }

    /// Folds in the next reload's diff, renumbering the marks after it and merging the ones it
    /// touches into one block.
    pub fn with(&self, diff: LineDiff) -> ChangeSet {
        let old_end = diff.first + diff.removed;
        let shift = |line: usize| line + diff.added - diff.removed;

        let mut start = diff.first;
        let mut end = diff.first + diff.added;
        let mut removed = diff.removed;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for m in &self.marks {
            let m_end = m.start + m.len;
            if m_end < diff.first {
                before.push(*m);
            } else if m.start > old_end {
                after.push(ChangeMark {
                    start: shift(m.start),
                    ..*m
                });
            } else {
                // Overlapping or touching: the new block absorbs it. Lines of it that the diff
                // replaced were never original lines, so they don't count as removed.
                let overlap = m_end.min(old_end).saturating_sub(m.start.max(diff.first));
                removed = removed - overlap + m.removed;
                start = start.min(m.start);
                end = end.max(if m_end >= old_end {
                    shift(m_end)
                } else {
                    diff.first + diff.added
                });
            }
        }
        let merged = ChangeMark {
            start,
            len: end - start,
            removed,
        };
        let marks = before
            .into_iter()
            .chain((merged.len > 0 || merged.removed > 0).then_some(merged))
            .chain(after)
            .collect();
        ChangeSet::from_marks(marks)
    }

    fn from_marks(marks: Vec<ChangeMark>) -> ChangeSet {
        let mut total = 0;
        let before = marks
            .iter()
            .map(|m| {
                let at = total;
                total += m.len;
                at
            })
            .collect();
        ChangeSet { marks, before }
    }

    /// The mark whose lines include `line`, if any.
    fn mark_of(&self, line: usize) -> Option<&ChangeMark> {
        let i = self.marks.partition_point(|m| m.start + m.len <= line);
        self.marks.get(i).filter(|m| m.start <= line && m.len > 0)
    }

    pub fn kind_of(&self, line: usize) -> Option<ChangeKind> {
        self.mark_of(line).map(|m| {
            if m.removed == 0 {
                ChangeKind::Added
            } else {
                ChangeKind::Changed
            }
        })
    }

    /// Lines were deleted, with nothing put in their place, just before `line`.
    pub fn deleted_before(&self, line: usize) -> bool {
        let i = self.marks.partition_point(|m| m.start < line);
        self.marks
            .get(i)
            .is_some_and(|m| m.start == line && m.len == 0)
    }

    /// The first line of the first mark after `line`, wrapping to the first mark.
    pub fn next_after(&self, line: i64) -> Option<usize> {
        let i = self.marks.partition_point(|m| m.start as i64 <= line);
        self.marks.get(i).or(self.marks.first()).map(|m| m.start)
    }
}

impl FilterIndex for ChangeSet {
    fn count(&self) -> usize {
        self.changed_lines()
    }

    fn get(&self, i: usize) -> i32 {
        let m = self.before.partition_point(|&b| b <= i) - 1;
        (self.marks[m].start + i - self.before[m]) as i32
    }

    fn index_at_or_after(&self, line: i32) -> usize {
        let line = line.max(0) as usize;
        let i = self.marks.partition_point(|m| m.start + m.len <= line);
        match self.marks.get(i) {
            Some(m) => self.before[i] + line.saturating_sub(m.start),
            None => self.changed_lines(),
        }
    }
}
