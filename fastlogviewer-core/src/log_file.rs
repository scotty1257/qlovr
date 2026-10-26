use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use memmap2::Mmap;

use crate::cancel::CancelToken;

const BLOCK_SHIFT: usize = 20; // 1,048,576 line-starts per block
const BLOCK_SIZE: usize = 1 << BLOCK_SHIFT;
const BLOCK_MASK: usize = BLOCK_SIZE - 1;

/// Per-line cap used when measuring line length, mirroring the original's 1,000,000-byte clamp so
/// one pathological line cannot blow out the horizontal scrollbar range.
const MAX_MEASURED_LINE: i64 = 1_000_000;

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;
const NL: u64 = 0x0A0A_0A0A_0A0A_0A0A;

/// Reads an aligned-or-not 8-byte window as a `u64`. Endianness is irrelevant: every consumer here
/// only asks "does this window contain a zero byte", which is order-independent.
#[inline(always)]
fn word_at(bytes: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap())
}

/// True if any of the eight bytes in `w` is zero (the classic SWAR zero-byte test).
#[inline(always)]
fn has_zero_byte(w: u64) -> bool {
    ((w.wrapping_sub(ONES)) & !w & HIGH) != 0
}

/// A memory-mapped log file plus its line-start index.
///
/// The mapping lives for as long as the `LogFile` does, so holding this in an `Arc` and handing
/// clones to workers is what keeps a scan from ever dereferencing an unmapped view — the original
/// C# had to cancel-and-join its threads before `Dispose`, and leaked the mapping when a worker
/// refused to stop in time. Here the last `Arc` to drop unmaps, whoever that is.
pub struct LogFile {
    path: PathBuf,
    length: u64,
    mmap: Option<Mmap>,

    starts: Vec<OnceLock<Box<[AtomicI64]>>>,
    count: AtomicUsize,

    max_line_bytes: AtomicI32, // for the horizontal scrollbar
    scanned: AtomicI64,        // indexing progress, bytes
    index_complete: AtomicBool,
}

impl LogFile {
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let length = std::fs::metadata(&path)?.len();
        let blocks = ((length >> BLOCK_SHIFT) + 2) as usize;

        let mut file = LogFile {
            path,
            length,
            mmap: None,
            starts: (0..blocks).map(|_| OnceLock::new()).collect(),
            count: AtomicUsize::new(0),
            max_line_bytes: AtomicI32::new(0),
            scanned: AtomicI64::new(0),
            index_complete: AtomicBool::new(false),
        };

        if length == 0 {
            file.index_complete.store(true, Ordering::Release);
            return Ok(file);
        }

        // The file is opened read-only and the mapping is read-only, which is the closest
        // equivalent to FileShare.ReadWrite|Delete: a live writer or a log rotation does not lock
        // us out, and we never write back.
        let handle = File::open(&file.path)?;
        // SAFETY: the mapping is read-only. A concurrent writer can change bytes under us, which
        // can only ever yield torn *content* in a line we render — never unsoundness, since every
        // read goes through a bounds-checked slice of the mapped length.
        file.mmap = Some(unsafe { Mmap::map(&handle)? });
        Ok(file)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn line_count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn max_line_bytes(&self) -> i32 {
        self.max_line_bytes.load(Ordering::Relaxed)
    }

    pub fn scanned(&self) -> i64 {
        self.scanned.load(Ordering::Relaxed)
    }

    pub fn index_complete(&self) -> bool {
        self.index_complete.load(Ordering::Acquire)
    }

    fn bytes(&self) -> &[u8] {
        match &self.mmap {
            Some(m) => m,
            None => &[],
        }
    }

    fn add_start(&self, offset: i64) {
        let i = self.count.load(Ordering::Relaxed); // single writer
        let block = self.starts[i >> BLOCK_SHIFT].get_or_init(|| {
            (0..BLOCK_SIZE)
                .map(|_| AtomicI64::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        block[i & BLOCK_MASK].store(offset, Ordering::Relaxed);
        self.count.store(i + 1, Ordering::Release); // publish last; readers see a complete entry
    }

    pub fn start_of(&self, line: usize) -> i64 {
        self.starts[line >> BLOCK_SHIFT]
            .get()
            .expect("block published before the count that exposes it")[line & BLOCK_MASK]
            .load(Ordering::Relaxed)
    }

    /// Scans the whole file for '\n' using the SWAR zero-byte trick. Run on a worker thread.
    pub fn build_index(&self, ct: &CancelToken) {
        if self.length == 0 {
            return;
        }

        let bytes = self.bytes();
        let len = bytes.len();
        let mut i = 0usize;
        let mut line_start: i64 = 0;
        let mut max_len: i32 = 0;

        self.add_start(0);

        while i + 8 <= len {
            let w = word_at(bytes, i) ^ NL; // '\n' bytes become 0x00
            if has_zero_byte(w) {
                for j in 0..8 {
                    if bytes[i + j] != b'\n' {
                        continue;
                    }
                    let s = (i + j + 1) as i64;
                    let ll = (s - 1 - line_start).min(MAX_MEASURED_LINE) as i32;
                    if ll > max_len {
                        max_len = ll;
                    }
                    line_start = s;
                    self.add_start(s);
                }
            }
            i += 8;
            if (i & 0x03FF_FFFF) == 0 {
                // every 64 MB
                self.scanned.store(i as i64, Ordering::Relaxed);
                self.max_line_bytes.store(max_len, Ordering::Relaxed);
                if ct.is_cancelled() {
                    return;
                }
            }
        }
        while i < len {
            if bytes[i] == b'\n' {
                let s = (i + 1) as i64;
                let ll = (s - 1 - line_start).min(MAX_MEASURED_LINE) as i32;
                if ll > max_len {
                    max_len = ll;
                }
                line_start = s;
                self.add_start(s);
            }
            i += 1;
        }

        let tail = (len as i64 - line_start).min(MAX_MEASURED_LINE) as i32;
        if tail > max_len {
            max_len = tail;
        }

        // A file ending in '\n' produced a phantom empty final line — drop it.
        let count = self.count.load(Ordering::Relaxed);
        if count > 1 && self.start_of(count - 1) == self.length as i64 {
            self.count.store(count - 1, Ordering::Release);
        }

        self.max_line_bytes.store(max_len, Ordering::Relaxed);
        self.scanned.store(len as i64, Ordering::Relaxed);
        self.index_complete.store(true, Ordering::Release);
    }

    /// Byte range of a line's content, excluding the '\n' and any '\r' before it.
    fn line_range(&self, line: usize, strip_trailing_newline: bool) -> Option<(usize, usize)> {
        let n = self.line_count();
        if line >= n {
            return None;
        }

        let s = self.start_of(line) as usize;
        let mut e = if line + 1 < n {
            self.start_of(line + 1) as usize - 1 // -1 skips the '\n'
        } else {
            self.length as usize
        };

        // The final line has no following start to subtract from, so if the file ends in a newline
        // that byte is still inside the range — drop it explicitly.
        let bytes = self.bytes();
        if strip_trailing_newline && e > s && bytes[e - 1] == b'\n' {
            e -= 1;
        }
        if e > s && bytes[e - 1] == b'\r' {
            e -= 1; // and the '\r'
        }
        Some((s, e.max(s)))
    }

    /// Decodes a line to a string, truncating at `max_bytes` and expanding tabs to four spaces.
    /// Invalid UTF-8 becomes U+FFFD, matching `Encoding.UTF8.GetString`.
    pub fn get_line(&self, line: usize, max_bytes: usize) -> String {
        let Some((s, e)) = self.line_range(line, false) else {
            return String::new();
        };
        let take = (e - s).min(max_bytes);
        if take == 0 {
            return String::new();
        }

        let text = String::from_utf8_lossy(&self.bytes()[s..s + take]);
        if text.contains('\t') {
            text.replace('\t', "    ")
        } else {
            text.into_owned()
        }
    }

    /// Copies the line's raw bytes (no '\r'/'\n') into `dest` and returns the count written,
    /// truncating at `dest.len()`. Zero-allocation companion to `get_line` — the template scanner
    /// calls this millions of times and must not create a string per line.
    pub fn copy_line(&self, line: usize, dest: &mut [u8]) -> usize {
        if dest.is_empty() {
            return 0;
        }
        let Some((s, e)) = self.line_range(line, true) else {
            return 0;
        };
        let take = (e - s).min(dest.len());
        if take == 0 {
            return 0;
        }
        dest[..take].copy_from_slice(&self.bytes()[s..s + take]);
        take
    }

    /// Byte-wise substring search, SWAR-accelerated on the first byte. Returns -1 if not found.
    pub fn find(&self, pattern: &[u8], from: i64) -> i64 {
        if self.length == 0 || pattern.is_empty() {
            return -1;
        }

        let bytes = self.bytes();
        let m = pattern.len();
        if m > bytes.len() {
            return -1;
        }

        let b0 = pattern[0];
        let splat = (b0 as u64).wrapping_mul(ONES);
        let limit = bytes.len() - m;
        let mut i = if from < 0 { 0usize } else { from as usize };

        while i <= limit {
            if i + 8 <= limit {
                let w = word_at(bytes, i) ^ splat;
                if !has_zero_byte(w) {
                    i += 8;
                    continue;
                }
            }
            if bytes[i] == b0 && bytes[i..i + m] == *pattern {
                return i as i64;
            }
            i += 1;
        }
        -1
    }

    pub fn line_of_offset(&self, offset: i64) -> usize {
        let mut lo: i64 = 0;
        let mut hi: i64 = self.line_count() as i64 - 1;
        let mut result = 0usize;
        while lo <= hi {
            let mid = ((lo + hi) >> 1) as usize;
            if self.start_of(mid) <= offset {
                result = mid;
                lo = mid as i64 + 1;
            } else {
                hi = mid as i64 - 1;
            }
        }
        result
    }
}
