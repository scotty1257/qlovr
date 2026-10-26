use std::sync::Arc;

use fastlogviewer_core::{ComplementIndex, FilterIndex};

/// A fixed, sorted list standing in for a finished `MatchIndex`.
struct Lines(Vec<i32>);

impl FilterIndex for Lines {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn get(&self, i: usize) -> i32 {
        self.0[i]
    }
    fn index_at_or_after(&self, line: i32) -> usize {
        self.0.partition_point(|&l| l < line)
    }
}

fn complement(matched: &[i32], total: usize) -> ComplementIndex {
    ComplementIndex::new(Arc::new(Lines(matched.to_vec())), total)
}

fn all(index: &ComplementIndex) -> Vec<i32> {
    (0..index.count()).map(|i| index.get(i)).collect()
}

#[test]
fn lists_every_line_the_source_does_not() {
    let index = complement(&[2, 3, 7], 10);
    assert_eq!(7, index.count());
    assert_eq!(vec![0, 1, 4, 5, 6, 8, 9], all(&index));
}

#[test]
fn index_at_or_after_skips_excluded_lines() {
    let index = complement(&[2, 3, 7], 10);
    assert_eq!(0, index.index_at_or_after(0));
    assert_eq!(2, index.index_at_or_after(2)); // 2 and 3 are excluded -> next kept is 4
    assert_eq!(2, index.index_at_or_after(3));
    assert_eq!(2, index.index_at_or_after(4));
    assert_eq!(5, index.index_at_or_after(7)); // -> 8
    assert_eq!(6, index.index_at_or_after(9));
    assert_eq!(7, index.index_at_or_after(10));
    assert_eq!(7, index.index_at_or_after(1000));
}

#[test]
fn edges_of_the_file() {
    assert_eq!(vec![1, 2], all(&complement(&[0, 3], 4)));
    assert_eq!(vec![0, 1, 2], all(&complement(&[], 3)));
    assert_eq!(0, complement(&[0, 1, 2], 3).count());
    assert_eq!(0, complement(&[], 0).count());
}
