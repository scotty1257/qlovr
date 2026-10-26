use std::sync::Arc;

use fastlogviewer_core::{FilterIndex, LogTemplate, TemplateTree, TokenBuffer};

fn feed(lines: &[&str]) -> TemplateTree {
    let tree = TemplateTree::default();
    let mut tb = TokenBuffer::new();
    for (i, line) in lines.iter().enumerate() {
        let b = line.as_bytes();
        tb.tokenize(b, 0, b.len());
        tree.ingest(&tb, b, i as i32);
    }
    tree
}

#[test]
fn disagreeing_positions_become_slots() {
    let t = feed(&[
        "GET /api/u/91 200 in 12ms",
        "GET /api/u/2201 200 in 8ms",
    ])
    .snapshot();

    assert_eq!(1, t.len());
    assert_eq!("GET /api/u/<*> 200 in <*>ms", t[0].render());
    assert_eq!(2, t[0].count());
    assert_eq!(2, t[0].slot_count());
}

#[test]
fn constant_tokens_stay_literal() {
    // The /5 is constant within its cluster and must not collapse into a slot.
    let t = feed(&["retrying u/91 attempt 1/5", "retrying u/92 attempt 3/5"]).snapshot();

    assert_eq!(1, t.len());
    assert_eq!("retrying u/<*> attempt <*>/5", t[0].render());
}

#[test]
fn different_token_counts_form_different_templates() {
    let t = feed(&["GET /api/u/91 200", "GET /api/orders 200"]).snapshot();
    assert_eq!(2, t.len());
}

#[test]
fn template_exposes_its_lines_as_a_filter_index() {
    let tree = TemplateTree::default();
    let mut tb = TokenBuffer::new();
    let lines = ["a 1", "b 1", "a 2", "a 3"];
    let file_lines = [10, 20, 30, 40];
    for (line, file_line) in lines.iter().zip(file_lines) {
        let b = line.as_bytes();
        tb.tokenize(b, 0, b.len());
        tree.ingest(&tb, b, file_line);
    }

    let a: Option<Arc<LogTemplate>> = tree.snapshot().into_iter().find(|t| t.count() == 3);

    let a = a.expect("the 'a <*>' cluster absorbed three lines");
    assert_eq!(3, a.count());
    assert_eq!(10, a.get(0));
    assert_eq!(30, a.get(1));
    assert_eq!(40, a.get(2));
    assert_eq!(1, a.index_at_or_after(30));
    assert_eq!(3, a.index_at_or_after(41));
}

#[test]
fn slot_positions_are_reported() {
    let t = feed(&[
        "GET /api/u/91 200 in 12ms",
        "GET /api/u/2201 200 in 8ms",
    ])
    .snapshot();
    assert_eq!(2, t[0].slot_token_indexes().len());
}

/// Pure-alpha token, so the tokenizer keeps it whole and each line lands in its own first-token
/// bucket — nothing can ever cluster.
fn letters(n: usize) -> String {
    let mut s = String::new();
    let mut n = n + 1;
    while n > 0 {
        s.insert(0, (b'a' + ((n - 1) % 26) as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

#[test]
fn overflows_into_unclustered_beyond_the_cap() {
    let tree = TemplateTree::default();
    let mut tb = TokenBuffer::new();

    for (line, n) in (0..TemplateTree::MAX_TEMPLATES + 200).enumerate() {
        let text = format!("{} tail", letters(n));
        let b = text.as_bytes();
        tb.tokenize(b, 0, b.len());
        tree.ingest(&tb, b, line as i32);
    }

    // MAX_TEMPLATES real templates, plus the single overflow bucket.
    assert!(tree.count() <= TemplateTree::MAX_TEMPLATES + 1);

    let saw_overflow = tree.snapshot().iter().any(|t| t.render() == "<unclustered>");
    assert!(saw_overflow);
}
