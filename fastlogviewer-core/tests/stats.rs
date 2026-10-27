use fastlogviewer_core::{bucket_counts, intersection_count, CancelToken, FilterIndex};

/// A fixed, sorted list of line numbers.
struct Lines(Vec<i32>);

impl FilterIndex for Lines {
    fn count(&self) -> usize {
        self.0.len()
    }
    fn get(&self, i: usize) -> i32 {
        self.0[i]
    }
    fn index_at_or_after(&self, line: i32) -> usize {
        self.0.partition_point(|v| *v < line)
    }
}

#[test]
fn bucket_counts_splits_the_file_evenly() {
    let lines = Lines(vec![0, 1, 2, 50, 99]);
    assert_eq!(vec![3, 0, 1, 1], bucket_counts(&lines, 100, 4));
}

#[test]
fn bucket_counts_handles_more_buckets_than_lines() {
    let lines = Lines(vec![0, 2]);
    let counts = bucket_counts(&lines, 3, 6);
    assert_eq!(6, counts.len());
    assert_eq!(2, counts.iter().sum::<usize>());
}

#[test]
fn bucket_counts_of_an_empty_file_are_zero() {
    assert_eq!(vec![0, 0], bucket_counts(&Lines(vec![]), 0, 2));
}

#[test]
fn intersection_count_counts_shared_lines() {
    let a = Lines(vec![1, 3, 5, 7, 9]);
    let b = Lines(vec![0, 3, 4, 9, 10]);
    assert_eq!(Some(2), intersection_count(&a, &b, &CancelToken::none()));
    assert_eq!(
        Some(0),
        intersection_count(&a, &Lines(vec![]), &CancelToken::none())
    );
}
