use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::OnceLock;

const BLOCK_SHIFT: usize = 16; // 65,536 entries per block
const BLOCK_SIZE: usize = 1 << BLOCK_SHIFT;
const BLOCK_MASK: usize = BLOCK_SIZE - 1;

/// Growable, append-only, sorted list of line numbers backed by fixed-size blocks so it never
/// reallocates/copies as it grows — safe to read (lock-free) while being appended to on a
/// background thread, same publish pattern as `LogFile`'s line index.
///
/// The block table is sized up front from a capacity hint and never resized, so a reader only ever
/// follows a `OnceLock` that is either empty or permanently populated.
pub struct LineNumberList {
    blocks: Vec<OnceLock<Box<[AtomicI32]>>>,
    count: AtomicUsize,
}

impl LineNumberList {
    pub fn new(capacity_hint: i64) -> Self {
        let blocks = ((capacity_hint >> BLOCK_SHIFT) + 2) as usize;
        LineNumberList {
            blocks: (0..blocks).map(|_| OnceLock::new()).collect(),
            count: AtomicUsize::new(0),
        }
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn get(&self, i: usize) -> i32 {
        self.blocks[i >> BLOCK_SHIFT]
            .get()
            .expect("block published before the count that exposes it")[i & BLOCK_MASK]
            .load(Ordering::Relaxed)
    }

    pub fn add(&self, line: i32) {
        let i = self.count.load(Ordering::Relaxed); // single writer: no race with itself
        let block = self.blocks[i >> BLOCK_SHIFT].get_or_init(|| {
            (0..BLOCK_SIZE)
                .map(|_| AtomicI32::new(0))
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        block[i & BLOCK_MASK].store(line, Ordering::Relaxed);
        self.count.store(i + 1, Ordering::Release); // publish last; readers see a complete entry
    }

    /// First entry index whose value is >= `value`, or `count()` if none.
    pub fn index_at_or_after(&self, value: i32) -> usize {
        let count = self.count();
        let mut lo: i64 = 0;
        let mut hi: i64 = count as i64 - 1;
        let mut res = count;
        while lo <= hi {
            let mid = ((lo + hi) >> 1) as usize;
            if self.get(mid) >= value {
                res = mid;
                hi = mid as i64 - 1;
            } else {
                lo = mid as i64 + 1;
            }
        }
        res
    }
}
