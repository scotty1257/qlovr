//! Numbers for the tag stats panel: where in the file a tag's matches fall, and how many of them
//! are among the lines currently shown.

use crate::cancel::CancelToken;
use crate::filter::FilterIndex;

/// Splits `0..total_lines` into `buckets` equal spans and counts the entries of `index` in each.
/// Costs two binary searches per bucket, so it is cheap enough to redo on every frame.
pub fn bucket_counts(index: &dyn FilterIndex, total_lines: usize, buckets: usize) -> Vec<usize> {
    if buckets == 0 || total_lines == 0 {
        return vec![0; buckets];
    }
    let mut counts = Vec::with_capacity(buckets);
    let mut before = 0;
    for bucket in 0..buckets {
        let end = bucket_start(bucket + 1, total_lines, buckets);
        let upto = index.index_at_or_after(end.min(i32::MAX as usize) as i32);
        counts.push(upto - before);
        before = upto;
    }
    counts
}

/// The first line of `bucket` when `total_lines` are split into `buckets` spans.
pub fn bucket_start(bucket: usize, total_lines: usize, buckets: usize) -> usize {
    (bucket as u128 * total_lines as u128 / buckets.max(1) as u128) as usize
}

/// How many lines are in both `a` and `b`, or `None` if cancelled first. Both must be sorted, as
/// every `FilterIndex` is.
pub fn intersection_count(
    a: &dyn FilterIndex,
    b: &dyn FilterIndex,
    ct: &CancelToken,
) -> Option<usize> {
    let (mut i, mut j, mut both) = (0usize, 0usize, 0usize);
    let (a_count, b_count) = (a.count(), b.count());
    let mut steps = 0u32;
    while i < a_count && j < b_count {
        let (va, vb) = (a.get(i), b.get(j));
        if va == vb {
            both += 1;
            i += 1;
            j += 1;
        } else if va < vb {
            i += 1;
        } else {
            j += 1;
        }
        steps = steps.wrapping_add(1);
        if steps % 65_536 == 0 && ct.is_cancelled() {
            return None;
        }
    }
    Some(both)
}
