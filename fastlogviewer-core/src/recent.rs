//! The files opened most recently, newest first, kept in %APPDATA%\FastLogViewer beside tags.json.

use std::fs;
use std::path::{Path, PathBuf};

/// How many files the list remembers.
pub const RECENT_LIMIT: usize = 10;

fn store_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("FastLogViewer"))
}

#[derive(Default)]
pub struct RecentFiles {
    pub paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// Corrupt or unreadable -> start empty, never crash on startup.
    pub fn load() -> Self {
        let paths = store_dir()
            .and_then(|dir| fs::read_to_string(dir.join("recent.json")).ok())
            .and_then(|json| serde_json::from_str::<Vec<PathBuf>>(&json).ok())
            .unwrap_or_default();
        let mut recent = RecentFiles { paths };
        recent.paths.truncate(RECENT_LIMIT);
        recent
    }

    /// Best-effort persistence; a failed save only forgets the list.
    pub fn save(&self) {
        let Some(dir) = store_dir() else { return };
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.paths) {
            let _ = fs::write(dir.join("recent.json"), json);
        }
    }

    /// Moves `path` to the front, dropping the oldest entry past the limit.
    pub fn add(&mut self, path: &Path) {
        self.remove(path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_LIMIT);
    }

    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| !same_path(p, path));
    }
}

/// Windows paths are case-insensitive, so `C:\Logs\app.log` and `c:\logs\APP.log` are one file.
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}
