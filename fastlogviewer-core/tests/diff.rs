mod common;

use common::write_temp;
use fastlogviewer_core::{
    diff_logs, CancelToken, ChangeKind, ChangeSet, FilterIndex, LineDiff, LogFile,
};

/// Diffs two texts the way a reload does: the old version only needs its bytes, the new one an
/// index.
fn diff(old: &str, new: &str) -> Option<LineDiff> {
    let (old_path, new_path) = (write_temp(old), write_temp(new));
    let result = {
        let old_file = LogFile::open(&old_path).unwrap();
        let new_file = LogFile::open(&new_path).unwrap();
        new_file.build_index(&CancelToken::none());
        diff_logs(&old_file, &new_file, &CancelToken::none())
    };
    std::fs::remove_file(old_path).unwrap();
    std::fs::remove_file(new_path).unwrap();
    result
}

fn d(first: usize, added: usize, removed: usize) -> Option<LineDiff> {
    Some(LineDiff {
        first,
        added,
        removed,
    })
}

#[test]
fn identical_files_have_no_diff() {
    assert_eq!(None, diff("a\nb\n", "a\nb\n"));
    assert_eq!(None, diff("", ""));
}

#[test]
fn appended_lines_are_added() {
    assert_eq!(d(2, 2, 0), diff("a\nb\n", "a\nb\nc\nd\n"));
    assert_eq!(d(0, 1, 0), diff("", "a\n"));
}

#[test]
fn appending_to_an_unterminated_last_line_changes_it() {
    assert_eq!(d(1, 2, 1), diff("a\nb", "a\nbc\nd\n"));
}

#[test]
fn truncation_removes_lines() {
    assert_eq!(d(1, 0, 2), diff("a\nb\nc\n", "a\n"));
    assert_eq!(d(0, 0, 2), diff("a\nb", ""));
}

#[test]
fn edits_in_the_middle_are_one_block() {
    assert_eq!(d(1, 1, 1), diff("a\nb\nc\n", "a\nB\nc\n"));
    assert_eq!(d(1, 0, 1), diff("a\nb\nc\n", "a\nc\n"));
    assert_eq!(d(1, 1, 0), diff("a\nc\n", "a\nb\nc\n"));
    // The tail repeats the head, so the comparisons from both ends must not overlap.
    assert_eq!(d(0, 1, 1), diff("ab", "abab"));
    assert_eq!(d(1, 3, 1), diff("x\ny\nz\n", "x\n1\n2\n3\nz\n"));
}

#[test]
fn changes_gather_over_reloads() {
    // Two appends merge into one added block.
    let set = ChangeSet::default()
        .with(LineDiff {
            first: 10,
            added: 3,
            removed: 0,
        })
        .with(LineDiff {
            first: 13,
            added: 2,
            removed: 0,
        });
    assert_eq!(1, set.marks().len());
    assert_eq!(5, set.changed_lines());
    assert_eq!(Some(ChangeKind::Added), set.kind_of(10));
    assert_eq!(Some(ChangeKind::Added), set.kind_of(14));
    assert_eq!(None, set.kind_of(15));
    assert_eq!(None, set.kind_of(9));

    // An edit before them shifts them down.
    let set = set.with(LineDiff {
        first: 2,
        added: 2,
        removed: 1,
    });
    assert_eq!(Some(ChangeKind::Changed), set.kind_of(2));
    assert_eq!(Some(ChangeKind::Changed), set.kind_of(3));
    assert_eq!(None, set.kind_of(4));
    assert_eq!(Some(ChangeKind::Added), set.kind_of(11));
    assert_eq!(Some(ChangeKind::Added), set.kind_of(15));
    assert_eq!(None, set.kind_of(16));
    assert_eq!((7, 1), (set.changed_lines(), set.removed_lines()));
}

#[test]
fn rewriting_added_lines_does_not_count_them_as_removed() {
    let set = ChangeSet::default()
        .with(LineDiff {
            first: 5,
            added: 3,
            removed: 0,
        })
        .with(LineDiff {
            first: 6,
            added: 1,
            removed: 1,
        });
    assert_eq!(0, set.removed_lines());
    assert_eq!(3, set.changed_lines());
    assert_eq!(Some(ChangeKind::Added), set.kind_of(6));
}

#[test]
fn deletions_are_marked_before_the_next_line() {
    let set = ChangeSet::default().with(LineDiff {
        first: 4,
        added: 0,
        removed: 2,
    });
    assert!(set.deleted_before(4));
    assert!(!set.deleted_before(3));
    assert_eq!(None, set.kind_of(4));
    assert_eq!(0, set.count());
}

#[test]
fn change_set_filters_to_changed_lines() {
    let set = ChangeSet::default()
        .with(LineDiff {
            first: 2,
            added: 2,
            removed: 0,
        })
        .with(LineDiff {
            first: 6,
            added: 0,
            removed: 1,
        })
        .with(LineDiff {
            first: 8,
            added: 3,
            removed: 0,
        });
    let lines: Vec<i32> = (0..set.count()).map(|i| set.get(i)).collect();
    assert_eq!(vec![2, 3, 8, 9, 10], lines);
    assert_eq!(0, set.index_at_or_after(0));
    assert_eq!(1, set.index_at_or_after(3));
    assert_eq!(2, set.index_at_or_after(4));
    assert_eq!(2, set.index_at_or_after(8));
    assert_eq!(4, set.index_at_or_after(10));
    assert_eq!(5, set.index_at_or_after(11));
}

#[test]
fn next_change_wraps() {
    let set = ChangeSet::default()
        .with(LineDiff {
            first: 2,
            added: 1,
            removed: 0,
        })
        .with(LineDiff {
            first: 8,
            added: 1,
            removed: 0,
        });
    assert_eq!(Some(2), set.next_after(-1));
    assert_eq!(Some(8), set.next_after(2));
    assert_eq!(Some(2), set.next_after(8));
    assert_eq!(None, ChangeSet::default().next_after(0));
}
