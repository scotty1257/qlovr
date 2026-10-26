use fastlogviewer_core::TokenBuffer;

fn tokens(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut tb = TokenBuffer::new();
    tb.tokenize(b, 0, b.len());
    (0..tb.count)
        .map(|i| String::from_utf8_lossy(tb.token(b, i)).into_owned())
        .collect()
}

#[test]
fn splits_on_class_transitions_and_punctuation() {
    assert_eq!(
        vec!["GET", "/", "api", "/", "u", "/", "91", "200", "in", "12", "ms"],
        tokens("GET /api/u/91 200 in 12ms")
    );
}

#[test]
fn records_whitespace_boundaries() {
    let b = b"a b/c";
    let mut tb = TokenBuffer::new();
    tb.tokenize(b, 0, b.len());
    // a | b | / | c   -> only "b" is preceded by whitespace
    assert_eq!(4, tb.count);
    assert!(!tb.space_before[0]);
    assert!(tb.space_before[1]);
    assert!(!tb.space_before[2]);
    assert!(!tb.space_before[3]);
}

#[test]
fn keeps_multi_byte_utf8_in_one_token() {
    assert_eq!(vec!["café", "ok"], tokens("café ok"));
}

#[test]
fn honours_start_offset_and_collapses_repeated_spaces() {
    let b = b"SKIPME   tail";
    let mut tb = TokenBuffer::new();
    tb.tokenize(b, 6, b.len());
    assert_eq!(1, tb.count);
    assert_eq!("tail", String::from_utf8_lossy(tb.token(b, 0)));
}

#[test]
fn truncates_beyond_max_tokens() {
    let line = "w ".repeat(100);
    let b = line.as_bytes();
    let mut tb = TokenBuffer::new();
    tb.tokenize(b, 0, b.len());
    assert_eq!(TokenBuffer::MAX_TOKENS, tb.count);
    assert!(tb.truncated);
}
