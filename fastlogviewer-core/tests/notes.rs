mod common;

use common::write_temp;
use fastlogviewer_core::{
    load_notes, notes_path, pin_line_notes, save_notes, CancelToken, LineNote, LoadedNotes,
    LogFile, NotesFile, PatternNote,
};

fn line_note(text: &str, line: usize) -> LineNote {
    LineNote {
        text: text.to_string(),
        line,
        note: "n".to_string(),
        created: 1,
        updated: 1,
    }
}

fn pins(content: &str, notes: &[LineNote]) -> Vec<Option<usize>> {
    let path = write_temp(content);
    let pinned = {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        pin_line_notes(&f, notes, &CancelToken::none()).unwrap()
    };
    std::fs::remove_file(path).unwrap();
    pinned
}

#[test]
fn notes_path_keeps_the_full_log_name() {
    let path = notes_path(std::path::Path::new("C:/logs/app.log"));
    assert_eq!(
        "app.log_markup.json",
        path.file_name().unwrap().to_str().unwrap()
    );
}

#[test]
fn a_note_stays_on_its_line_when_the_text_is_still_there() {
    assert_eq!(vec![Some(1)], pins("a\nb\nc\n", &[line_note("b", 1)]));
}

#[test]
fn a_note_follows_its_text_to_the_nearest_identical_line() {
    // "x" now sits at 0 and 5; the note was written at 4.
    let content = "x\na\nb\nc\nd\nx\n";
    assert_eq!(vec![Some(5)], pins(content, &[line_note("x", 4)]));
    assert_eq!(vec![Some(0)], pins(content, &[line_note("x", 1)]));
}

#[test]
fn a_note_whose_text_is_gone_is_dropped() {
    assert_eq!(vec![None], pins("a\nb\n", &[line_note("gone", 0)]));
}

#[test]
fn tabs_in_a_line_match_the_text_as_shown() {
    assert_eq!(vec![Some(1)], pins("a\nx\ty\n", &[line_note("x    y", 0)]));
}

#[test]
fn notes_round_trip_and_an_empty_set_removes_the_file() {
    let log = write_temp("a\n");
    let mut notes = NotesFile::for_log(&log);
    notes.line_notes.push(line_note("a", 0));
    notes.pattern_notes.push(PatternNote {
        pattern: "GET <*>".to_string(),
        note: "p".to_string(),
        created: 2,
        updated: 2,
    });
    save_notes(&log, &notes).unwrap();

    match load_notes(&log) {
        LoadedNotes::Notes(loaded) => assert_eq!(notes, loaded),
        _ => panic!("expected the saved notes back"),
    }

    save_notes(&log, &NotesFile::for_log(&log)).unwrap();
    assert!(!notes_path(&log).exists());
    std::fs::remove_file(log).unwrap();
}

#[test]
fn a_notes_file_written_for_another_log_is_not_ours() {
    let log = write_temp("a\n");
    let mut other = NotesFile::for_log(std::path::Path::new("other.log"));
    other.line_notes.push(line_note("a", 0));
    std::fs::write(notes_path(&log), serde_json_string(&other)).unwrap();

    assert!(matches!(load_notes(&log), LoadedNotes::Foreign));
    std::fs::remove_file(notes_path(&log)).unwrap();
    std::fs::remove_file(log).unwrap();
}

#[test]
fn a_missing_notes_file_loads_as_empty() {
    let log = write_temp("a\n");
    match load_notes(&log) {
        LoadedNotes::Notes(notes) => assert!(notes.is_empty()),
        _ => panic!("expected empty notes"),
    }
    std::fs::remove_file(log).unwrap();
}

fn serde_json_string(notes: &NotesFile) -> String {
    let dir = std::env::temp_dir().join(format!("flv-notes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("other.log");
    save_notes(&log, notes).unwrap();
    let json = std::fs::read_to_string(notes_path(&log)).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    json
}
