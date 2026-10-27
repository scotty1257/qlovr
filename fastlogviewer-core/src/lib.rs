// fastlogviewer-core — the indexing, search and clustering engine behind FastLogViewer.
//
// Architecture (this is the whole trick):
//   1. Memory-map the file        -> zero copy, zero load time, OS pages it in on demand.
//   2. Index line starts in bg    -> SWAR scan for '\n', ~2-4 GB/s, no allocations per line.
//   3. Index matches in bg        -> ONE scan feeds highlight + F3 + filter. UI never blocks.
//   4. Draw only visible lines    -> see the `fastlogviewer` binary crate.
//
// Threading model: every index type here is built on a worker thread and read concurrently by the
// UI. The append-only structures publish their element *before* their count (`Release` store on the
// count, `Acquire` load on the read), so a reader either does not see an entry yet or sees it whole.
// That is the same publish discipline the original C# used with `Volatile.Write`, made explicit.

pub mod cancel;
pub mod copy;
pub mod filter;
pub mod line_numbers;
pub mod log_file;
pub mod match_index;
pub mod matcher;
pub mod notes;
pub mod slot_filter;
pub mod slot_query;
pub mod stats;
pub mod tags;
pub mod templates;

pub use cancel::CancelToken;
pub use copy::{copy_text_of, join_lines, join_texts, parse_line_range, Joined, COPY_LIMIT_BYTES};
pub use filter::{ComplementIndex, FilterIndex, IntersectionIndex, UnionFilterIndex};
pub use line_numbers::LineNumberList;
pub use log_file::LogFile;
pub use match_index::MatchIndex;
pub use matcher::Matcher;
pub use notes::{
    load_notes, notes_path, pin_line_notes, save_notes, LineNote, LoadedNotes, NotesFile,
    PatternNote,
};
pub use slot_filter::SlotFilterIndex;
pub use slot_query::SlotQuery;
pub use stats::{bucket_counts, bucket_start, intersection_count};
pub use tags::{TagDef, TagStore, TAG_PALETTE};
pub use templates::{
    LogTemplate, LogViewMode, PrefixKind, PrefixShape, TemplateIndex, TemplateTree, TokenBuffer,
};

/// Longest line the viewer will decode. Matches the original's 32 KB cap.
pub const MAX_LINE_BYTES: usize = 32768;
