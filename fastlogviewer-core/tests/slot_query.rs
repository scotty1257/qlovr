use fastlogviewer_core::SlotQuery;

/// Slot values as the scanner produces them: one entry per slot, `None` when the line was too
/// short to carry that slot at all.
fn values(v: &[&str]) -> Vec<Option<String>> {
    v.iter().map(|s| Some(s.to_string())).collect()
}

#[test]
fn parses_and_evaluates_numeric_comparison() {
    let q = SlotQuery::parse("slot2 > 2000", 2);
    assert!(q.error.is_none());
    assert!(q.evaluate(&values(&["x", "8391"])));
    assert!(!q.evaluate(&values(&["x", "12"])));
}

#[test]
fn and_binds_tighter_than_or() {
    let q = SlotQuery::parse("slot1 >= 100 AND slot1 < 500 OR slot1 = 9999", 1);
    assert!(q.error.is_none());
    assert!(q.evaluate(&values(&["250"])));
    assert!(!q.evaluate(&values(&["800"])));
    assert!(q.evaluate(&values(&["9999"])));
}

#[test]
fn supports_quoted_string_equality() {
    let q = SlotQuery::parse("slot1 = \"read timeout\"", 1);
    assert!(q.error.is_none());
    assert!(q.evaluate(&values(&["read timeout"])));
    assert!(!q.evaluate(&values(&["other"])));
}

#[test]
fn ordering_operator_on_non_numeric_literal_is_a_parse_error() {
    let q = SlotQuery::parse("slot1 > abc", 1);
    let error = q.error.expect("ordering against a literal must not parse");
    assert!(error.contains("numeric"), "unexpected message: {error}");
}

#[test]
fn slot_index_out_of_range_is_a_parse_error() {
    let q = SlotQuery::parse("slot3 = 1", 2);
    let error = q.error.expect("slot 3 does not exist on a 2-slot template");
    assert!(error.contains("slot"), "unexpected message: {error}");
}

#[test]
fn malformed_input_reports_an_error() {
    assert!(SlotQuery::parse("slot1", 1).error.is_some());
    assert!(SlotQuery::parse("slot1 ~ 5", 1).error.is_some());
    assert!(SlotQuery::parse("banana", 1).error.is_some());
    assert!(SlotQuery::parse("slot1 = 5 AND", 1).error.is_some());
    assert!(SlotQuery::parse("", 1).error.is_some());
}

#[test]
fn missing_slot_value_does_not_match() {
    let q = SlotQuery::parse("slot1 = 5", 1);
    assert!(!q.evaluate(&[None]));
}

#[test]
fn unspaced_operators_parse_the_same() {
    let q = SlotQuery::parse("slot1>=100", 1);
    assert!(q.error.is_none());
    assert!(q.evaluate(&values(&["100"])));
    assert!(!q.evaluate(&values(&["99"])));
}

#[test]
fn recognises_slot_query_text() {
    assert!(SlotQuery::looks_like_slot_query("slot1 > 5"));
    assert!(SlotQuery::looks_like_slot_query("  SLOT2 = x"));
    assert!(!SlotQuery::looks_like_slot_query("slothful beast"));
    assert!(!SlotQuery::looks_like_slot_query("error"));
    assert!(!SlotQuery::looks_like_slot_query(""));
}
