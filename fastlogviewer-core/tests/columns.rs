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
    let shape = detect_columns(&lines(JAVA_STYLE)).expect("a column layout");
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
    let shape = detect_columns(&lines(JAVA_STYLE)).unwrap();
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
    let shape = detect_columns(&lines(JAVA_STYLE)).unwrap();
    assert!(shape
        .render("    at com.acme.Server.run(Server.java:42)")
        .is_none());
}

#[test]
fn free_text_has_no_columns() {
    let text = "the quick brown fox\njumps over the lazy dog\nand runs away\nfar from here";
    assert!(detect_columns(&lines(text)).is_none());
}

#[test]
fn level_only_logs_still_get_two_columns() {
    let text = "INFO starting\nWARN low disk\nERROR failed to write\nINFO done";
    let shape = detect_columns(&lines(text)).expect("level and message");
    assert_eq!(vec!["level", "message"], shape.headers);
}
