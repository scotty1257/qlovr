mod common;

use common::write_temp;
use fastlogviewer_core::{join_lines, parse_line_range, CancelToken, LogFile};

#[test]
fn parse_line_range_accepts_common_spellings() {
    assert_eq!(Ok((9, 19)), parse_line_range("10-20", 100));
    assert_eq!(Ok((9, 19)), parse_line_range(" 10 - 20 ", 100));
    assert_eq!(Ok((9, 19)), parse_line_range("10..20", 100));
    assert_eq!(Ok((9, 9)), parse_line_range("10", 100));
    assert_eq!(Ok((999, 1999)), parse_line_range("1,000-2,000", 5000));
}

#[test]
fn parse_line_range_swaps_reversed_ends_and_clamps_past_the_end() {
    assert_eq!(Ok((9, 19)), parse_line_range("20-10", 100));
    assert_eq!(Ok((89, 99)), parse_line_range("90-500", 100));
}

#[test]
fn parse_line_range_rejects_nonsense() {
    assert!(parse_line_range("", 100).is_err());
    assert!(parse_line_range("abc", 100).is_err());
    assert!(parse_line_range("0-5", 100).is_err());
    assert!(parse_line_range("200-300", 100).is_err());
    assert!(parse_line_range("1-2-3", 100).is_err());
    assert!(parse_line_range("1", 0).is_err());
}

#[test]
fn join_lines_joins_with_newlines() {
    let path = write_temp("alpha\r\nbeta\ngamma\n");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());

        let joined = join_lines(&f, [0, 2].into_iter(), 1 << 20);
        assert_eq!("alpha\ngamma", joined.text);
        assert_eq!(2, joined.lines);
        assert!(!joined.truncated);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn join_lines_stops_at_the_byte_limit() {
    let path = write_temp("aaaa\nbbbb\ncccc\n");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());

        let joined = join_lines(&f, 0..3, 10);
        assert_eq!("aaaa\nbbbb", joined.text);
        assert_eq!(2, joined.lines);
        assert!(joined.truncated);
    }
    std::fs::remove_file(path).unwrap();
}
