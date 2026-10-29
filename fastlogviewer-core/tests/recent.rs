use std::path::{Path, PathBuf};

use fastlogviewer_core::{RecentFiles, RECENT_LIMIT};

#[test]
fn add_puts_newest_first() {
    let mut recent = RecentFiles::default();
    recent.add(Path::new(r"C:\logs\a.log"));
    recent.add(Path::new(r"C:\logs\b.log"));
    assert_eq!(
        recent.paths,
        vec![PathBuf::from(r"C:\logs\b.log"), PathBuf::from(r"C:\logs\a.log")]
    );
}

#[test]
fn reopening_moves_to_front_without_duplicating() {
    let mut recent = RecentFiles::default();
    recent.add(Path::new(r"C:\logs\a.log"));
    recent.add(Path::new(r"C:\logs\b.log"));
    recent.add(Path::new(r"c:\LOGS\A.log"));
    assert_eq!(
        recent.paths,
        vec![PathBuf::from(r"c:\LOGS\A.log"), PathBuf::from(r"C:\logs\b.log")]
    );
}

#[test]
fn add_keeps_only_the_limit() {
    let mut recent = RecentFiles::default();
    for i in 0..RECENT_LIMIT + 3 {
        recent.add(&PathBuf::from(format!(r"C:\logs\{i}.log")));
    }
    assert_eq!(recent.paths.len(), RECENT_LIMIT);
    assert_eq!(
        recent.paths[0],
        PathBuf::from(format!(r"C:\logs\{}.log", RECENT_LIMIT + 2))
    );
}

#[test]
fn remove_ignores_case() {
    let mut recent = RecentFiles::default();
    recent.add(Path::new(r"C:\logs\a.log"));
    recent.remove(Path::new(r"C:\LOGS\a.LOG"));
    assert!(recent.paths.is_empty());
}
