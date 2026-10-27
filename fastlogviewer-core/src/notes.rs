//! Notes ("markup") a user attaches to a log: on single lines, and on patterns found by
//! clustering. They live beside the log in `<log file name>_markup.json`.
//!
//! A line note is pinned to the line's *text*, not its number. When the log is rewritten the note
//! follows its text to the nearest identical line, and is dropped when no line has that text any
//! more: a note must never end up describing some other line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cancel::CancelToken;
use crate::copy::copy_text_of;
use crate::log_file::LogFile;

const NOTES_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LineNote {
    /// The line's text as the viewer shows it (tabs expanded), which is what pins the note.
    pub text: String,
    /// 0-based line the text was last found on.
    pub line: usize,
    pub note: String,
    /// Unix seconds.
    pub created: u64,
    pub updated: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PatternNote {
    /// The pattern as `LogTemplate::render` writes it.
    pub pattern: String,
    pub note: String,
    pub created: u64,
    pub updated: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NotesFile {
    pub version: u32,
    /// File name of the log these notes belong to; a notes file naming another log is left alone.
    pub log_name: String,
    #[serde(default)]
    pub line_notes: Vec<LineNote>,
    #[serde(default)]
    pub pattern_notes: Vec<PatternNote>,
}

impl NotesFile {
    pub fn for_log(log: &Path) -> NotesFile {
        NotesFile {
            version: NOTES_VERSION,
            log_name: file_name(log),
            line_notes: Vec::new(),
            pattern_notes: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.line_notes.is_empty() && self.pattern_notes.is_empty()
    }
}

pub enum LoadedNotes {
    Notes(NotesFile),
    /// The notes file exists but belongs to another log or can't be read; it must not be
    /// overwritten.
    Foreign,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `app.log` -> `app.log_markup.json`, in the log's directory. The extension stays in so
/// `app.log` and `app.txt` never share notes.
pub fn notes_path(log: &Path) -> PathBuf {
    log.with_file_name(format!("{}_markup.json", file_name(log)))
}

pub fn load_notes(log: &Path) -> LoadedNotes {
    let json = match fs::read_to_string(notes_path(log)) {
        Ok(json) => json,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return LoadedNotes::Notes(NotesFile::for_log(log))
        }
        Err(_) => return LoadedNotes::Foreign,
    };
    match serde_json::from_str::<NotesFile>(&json) {
        Ok(notes) if notes.log_name == file_name(log) => LoadedNotes::Notes(notes),
        _ => LoadedNotes::Foreign,
    }
}

/// Writes the notes beside the log, or removes the notes file when there are none.
pub fn save_notes(log: &Path, notes: &NotesFile) -> io::Result<()> {
    let path = notes_path(log);
    if notes.is_empty() {
        return match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let json = serde_json::to_string_pretty(notes).map_err(io::Error::other)?;
    fs::write(path, json)
}

/// Finds where each note's text is now: its saved line if the text is still there, otherwise the
/// nearest line with identical text, otherwise `None`. The line index must be complete. Returns
/// `None` if cancelled.
pub fn pin_line_notes(
    file: &LogFile,
    notes: &[LineNote],
    ct: &CancelToken,
) -> Option<Vec<Option<usize>>> {
    let count = file.line_count();
    let mut pins: Vec<Option<usize>> = notes
        .iter()
        .map(|n| (n.line < count && copy_text_of(file, n.line) == n.text).then_some(n.line))
        .collect();

    let lost: Vec<usize> = (0..notes.len()).filter(|&i| pins[i].is_none()).collect();
    if lost.is_empty() {
        return Some(pins);
    }

    // One pass over the file. A line whose raw bytes are longer than the longest lost text can't
    // match (expanding tabs only makes text longer), so most lines are skipped without decoding.
    let longest = lost.iter().map(|&i| notes[i].text.len()).max().unwrap_or(0);
    let mut buf = vec![0u8; longest + 1];
    for line in 0..count {
        if line % 65_536 == 0 && ct.is_cancelled() {
            return None;
        }
        let len = file.copy_line(line, &mut buf);
        if len > longest {
            continue;
        }
        let text = String::from_utf8_lossy(&buf[..len]).replace('\t', "    ");
        for &i in &lost {
            if notes[i].text != text {
                continue;
            }
            let distance = line.abs_diff(notes[i].line);
            if pins[i].is_none_or(|best| distance < best.abs_diff(notes[i].line)) {
                pins[i] = Some(line);
            }
        }
    }
    Some(pins)
}
