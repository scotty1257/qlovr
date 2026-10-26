use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `content` to a uniquely-named file in the temp directory and returns the path. The
/// counter plus pid keeps parallel test threads from colliding, which is what
/// `Path.GetRandomFileName` bought the original.
pub fn write_temp(content: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "flv-test-{}-{}.log",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, content).expect("temp file is writable");
    path
}
