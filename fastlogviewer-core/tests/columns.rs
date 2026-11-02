mod common;

use fastlogviewer_core::detect_columns;

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

const JAVA_STYLE: &str = "\
2024-03-01 10:00:00.120 INFO [main] com.acme.Server - Starting server on port 8080
2024-03-01 10:00:00.250 DEBUG [main] com.acme.Config - Loaded 12 settings
2024-03-01 10:00:01.003 WARN [worker-1] com.acme.Pool - Pool nearly full
2024-03-01 10:00:02.999 ERROR [worker-2] com.acme.Db - Connection refused
2024-03-01 10:00:03.100 INFO [main] com.acme.Server - Ready";

#[test]
fn finds_time_level_thread_source_and_message() {
    let shape = detect_columns(&lines(JAVA_STYLE), None).expect("a column layout");
    assert_eq!(
        vec!["time", "level", "col 3", "col 4", "message"],
        shape.headers
    );
    let split = shape
        .split("2024-03-01 10:00:03.100 INFO [main] com.acme.Server - Ready")
        .unwrap();
    assert_eq!(
        vec![
            "2024-03-01 10:00:03.100",
            "INFO",
            "[main]",
            "com.acme.Server",
            "Ready"
        ],
        split
    );
}

#[test]
fn renders_columns_padded_to_a_common_width() {
    let shape = detect_columns(&lines(JAVA_STYLE), None).unwrap();
    let a = shape
        .render("2024-03-01 10:00:00.120 INFO [main] com.acme.Server - Starting")
        .unwrap();
    let b = shape
        .render("2024-03-01 10:00:02.999 ERROR [worker-2] com.acme.Db - Refused")
        .unwrap();
    assert_eq!(a.find("Starting"), b.find("Refused"));
}

#[test]
fn a_line_that_does_not_fit_is_left_alone() {
    let shape = detect_columns(&lines(JAVA_STYLE), None).unwrap();
    assert!(shape
        .render("    at com.acme.Server.run(Server.java:42)")
        .is_none());
}

#[test]
fn free_text_has_no_columns() {
    let text = "the quick brown fox\njumps over the lazy dog\nand runs away\nfar from here";
    assert!(detect_columns(&lines(text), None).is_none());
}

#[test]
fn level_only_logs_still_get_two_columns() {
    let text = "INFO starting\nWARN low disk\nERROR failed to write\nINFO done";
    let shape = detect_columns(&lines(text), None).expect("level and message");
    assert_eq!(vec!["level", "message"], shape.headers);
}

const JSON_LINES: &str = r#"{"time": "2024-03-01T10:00:00Z", "msg": "Starting\nserver", "level": "info", "port": 8080}
{"time": "2024-03-01T10:00:01Z", "level": "warn", "msg": "Pool nearly full"}
{"time": "2024-03-01T10:00:02Z", "level": "error", "msg": "Refused", "error": {"code": 61}}
{"time": "2024-03-01T10:00:03Z", "level": "info", "msg": "Ready"}"#;

#[test]
fn json_lines_get_a_column_per_key_with_the_message_last() {
    let shape = detect_columns(&lines(JSON_LINES), None).expect("a JSON layout");
    assert_eq!(vec!["time", "level", "port", "error", "msg"], shape.headers);
}

#[test]
fn json_values_are_unquoted_and_missing_keys_are_empty() {
    let shape = detect_columns(&lines(JSON_LINES), None).unwrap();
    let split = shape
        .split(r#"{"time": "t", "msg": "a\nb", "level": "error", "error": {"code": 61}}"#)
        .unwrap();
    assert_eq!(vec!["t", "error", "", r#"{"code":61}"#, "a b"], split);
}

#[test]
fn json_lines_render_aligned() {
    let shape = detect_columns(&lines(JSON_LINES), None).unwrap();
    let a = shape
        .render(r#"{"time": "2024-03-01T10:00:00Z", "level": "info", "msg": "Starting"}"#)
        .unwrap();
    let b = shape
        .render(r#"{"level": "error", "msg": "Refused", "port": 1}"#)
        .unwrap();
    assert_eq!(a.find("Starting"), b.find("Refused"));
}

#[test]
fn a_json_line_with_an_unseen_key_is_left_alone() {
    let shape = detect_columns(&lines(JSON_LINES), None).unwrap();
    assert!(shape.render(r#"{"level": "info", "extra": 1}"#).is_none());
    assert!(shape.render("not json at all").is_none());
}

const PYTHON_STYLE: &str = "\
INFO:root:Starting server
DEBUG:app.config:Loaded 12 settings
WARNING:app.pool:Pool nearly full: 9 of 10
ERROR:app.db:Connection refused
INFO:root:Ready";

#[test]
fn detection_tries_other_delimiters_when_whitespace_finds_nothing() {
    let shape = detect_columns(&lines(PYTHON_STYLE), None).expect("a colon layout");
    assert_eq!(vec!["level", "col 2", "message"], shape.headers);
}

#[test]
fn the_last_column_keeps_the_rest_of_the_line_delimiters_and_all() {
    let shape = detect_columns(&lines(PYTHON_STYLE), Some(":")).unwrap();
    let split = shape
        .split("WARNING:app.pool:Pool nearly full: 9 of 10")
        .unwrap();
    assert_eq!(
        vec!["WARNING", "app.pool", "Pool nearly full: 9 of 10"],
        split
    );
}

#[test]
fn a_leading_timestamp_stays_whole_when_its_text_holds_the_delimiter() {
    let text = "\
2024-03-01 10:00:00.120:INFO:app:Starting
2024-03-01 10:00:01.250:WARN:pool:Nearly full
2024-03-01 10:00:02.999:ERROR:db:Refused";
    let shape = detect_columns(&lines(text), Some(":")).expect("a colon layout");
    assert_eq!(vec!["time", "level", "col 3", "message"], shape.headers);
    let split = shape
        .split("2024-03-01 10:00:03.100:INFO:app:Ready")
        .unwrap();
    assert_eq!(
        vec!["2024-03-01 10:00:03.100", "INFO", "app", "Ready"],
        split
    );
}

#[test]
fn a_typed_delimiter_splits_and_trims_fields() {
    let text = "a | b | first\nc | d | second\ne | f | third";
    let shape = detect_columns(&lines(text), Some("|")).unwrap();
    assert_eq!(3, shape.headers.len());
    assert_eq!(
        vec!["c", "d", "second"],
        shape.split("c | d | second").unwrap()
    );
}

#[test]
fn a_line_with_too_few_fields_does_not_fit() {
    let shape = detect_columns(&lines(PYTHON_STYLE), Some(":")).unwrap();
    assert!(shape.split("    at app.run(app.py)").is_none());
    assert!(shape.split("INFO only one colon").is_none());
}

#[test]
fn a_space_delimiter_takes_a_run_of_spaces_as_one() {
    let text = "alpha    one   first\nbeta  two second\ngamma three    third";
    let shape = detect_columns(&lines(text), Some(" ")).unwrap();
    assert_eq!(
        vec!["beta", "two", "second"],
        shape.split("beta  two second").unwrap()
    );
}

#[test]
fn a_delimiter_most_lines_lack_finds_no_layout() {
    assert!(detect_columns(&lines(JAVA_STYLE), Some(";")).is_none());
}

#[test]
fn the_fit_index_lists_only_lines_that_fit() {
    use std::sync::Arc;

    use fastlogviewer_core::{CancelToken, ColumnFitIndex, FilterIndex, LogFile};

    let path = common::write_temp("INFO:root:a\n  traceback\nWARNING:app:b\nnot a fit\n");
    {
        let file = LogFile::open(&path).unwrap();
        file.build_index(&CancelToken::none());
        let shape = detect_columns(&lines(PYTHON_STYLE), Some(":")).unwrap();
        let index = ColumnFitIndex::new(Arc::new(shape), file.length());
        index.build(&file, &CancelToken::none());
        assert!(index.is_complete());
        let fit: Vec<i32> = (0..index.count()).map(|i| index.get(i)).collect();
        assert_eq!(vec![0, 2], fit);
    }
    std::fs::remove_file(path).unwrap();
}
