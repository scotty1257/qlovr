//! Splitting structured log lines into columns: timestamp, level, thread and so on, then the
//! message.
//!
//! The layout is guessed from a sample. A leading timestamp is recognised with the same
//! `PrefixShape` clustering uses; after it, each whitespace-separated field becomes a column for
//! as long as most sampled lines agree on what kind of field sits in that position. Everything
//! after the last agreed field is the message. A line that doesn't fit the layout (a stack
//! trace, a wrapped message) gets no columns at all.

use crate::templates::{PrefixKind, PrefixShape};

/// Share of sampled lines that must agree on a field's kind for it to become a column.
const AGREEMENT: f64 = 0.8;
/// Most leading fields considered.
const MAX_FIELDS: usize = 6;
/// Widest a column is padded to; longer values just push that one line's message along.
const MAX_WIDTH: usize = 40;
/// Space between rendered columns.
pub const COLUMN_GAP: usize = 2;

const LEVELS: [&str; 11] = [
    "TRACE", "DEBUG", "DBG", "INFO", "NOTICE", "WARN", "WARNING", "ERROR", "ERR", "FATAL",
    "CRITICAL",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FieldKind {
    Level,
    /// `[main]`, `[worker 3]`
    Bracketed,
    Number,
    /// `com.acme.Server`
    Dotted,
    /// A lone `-`, `|` or `:` between fields; matched but not shown as a column.
    Separator,
    Other,
}

#[derive(Clone, Debug)]
pub struct ColumnShape {
    prefix: PrefixShape,
    fields: Vec<FieldKind>,
    /// One per shown column, the message last.
    pub headers: Vec<String>,
    /// Padded width of each shown column but the message.
    pub widths: Vec<usize>,
}

/// Guesses a column layout from sample lines, or `None` when they don't share one worth showing
/// (at least one column besides the message).
pub fn detect_columns(sample: &[String]) -> Option<ColumnShape> {
    if sample.is_empty() {
        return None;
    }
    let bytes: Vec<Vec<u8>> = sample.iter().map(|l| l.as_bytes().to_vec()).collect();
    let prefix = PrefixShape::sniff(&bytes);
    let needed = (sample.len() as f64 * AGREEMENT).ceil() as usize;

    // Field kinds per line, after the timestamp; lines without the timestamp drop out.
    let kinds: Vec<Vec<FieldKind>> = sample
        .iter()
        .filter_map(|line| {
            let rest = after_timestamp(prefix, line)?;
            Some(
                fields(rest, MAX_FIELDS)
                    .iter()
                    .map(|f| kind_of(f))
                    .collect(),
            )
        })
        .collect();
    if kinds.len() < needed {
        return None;
    }

    let mut chosen: Vec<FieldKind> = Vec::new();
    for position in 0..MAX_FIELDS {
        let mut tally: Vec<(FieldKind, usize)> = Vec::new();
        for line in &kinds {
            if line.len() <= position + 1 || line[..position] != chosen[..] {
                continue; // needs a message after the field, and the fields before it to agree
            }
            match tally.iter_mut().find(|(k, _)| *k == line[position]) {
                Some((_, n)) => *n += 1,
                None => tally.push((line[position], 1)),
            }
        }
        let Some(&(kind, n)) = tally.iter().max_by_key(|(_, n)| *n) else {
            break;
        };
        if kind == FieldKind::Other || n < needed {
            break;
        }
        chosen.push(kind);
    }

    let mut headers = Vec::new();
    if prefix.kind != PrefixKind::None {
        headers.push("time".to_string());
    }
    for kind in chosen.iter().filter(|k| **k != FieldKind::Separator) {
        headers.push(match kind {
            FieldKind::Level => "level".to_string(),
            _ => format!("col {}", headers.len() + 1),
        });
    }
    if headers.is_empty() {
        return None;
    }
    headers.push("message".to_string());

    let mut shape = ColumnShape {
        prefix,
        fields: chosen,
        widths: headers[..headers.len() - 1]
            .iter()
            .map(|h| h.len())
            .collect(),
        headers,
    };
    for line in sample {
        if let Some(split) = shape.split(line) {
            for (width, value) in shape.widths.iter_mut().zip(&split) {
                *width = (*width).max(value.chars().count()).min(MAX_WIDTH);
            }
        }
    }
    Some(shape)
}

impl ColumnShape {
    /// The line's shown columns, the message last, or `None` when it doesn't fit the layout.
    pub fn split<'a>(&self, line: &'a str) -> Option<Vec<&'a str>> {
        let mut columns = Vec::with_capacity(self.headers.len());
        let rest = if self.prefix.kind == PrefixKind::None {
            line
        } else {
            let ts = self.prefix.measure_timestamp(line.as_bytes(), line.len());
            columns.push(line.get(..ts)?.trim_end());
            after_timestamp(self.prefix, line)?
        };

        let mut rest = rest.trim_start();
        for kind in &self.fields {
            let (field, after) = next_field(rest)?;
            if kind_of(field) != *kind {
                return None;
            }
            if *kind != FieldKind::Separator {
                columns.push(field);
            }
            rest = after.trim_start();
        }
        columns.push(rest);
        Some(columns)
    }

    /// The line laid out as padded columns, or `None` when it doesn't fit the layout.
    pub fn render(&self, line: &str) -> Option<String> {
        let split = self.split(line)?;
        let mut out = String::with_capacity(line.len() + 32);
        for (value, width) in split.iter().zip(&self.widths) {
            out.push_str(value);
            let pad = width.saturating_sub(value.chars().count()) + COLUMN_GAP;
            out.extend(std::iter::repeat_n(' ', pad));
        }
        out.push_str(split.last().copied().unwrap_or(""));
        Some(out)
    }

    /// Char offset where each shown column starts in a rendered line.
    pub fn offsets(&self) -> Vec<usize> {
        let mut at = 0;
        let mut offsets = Vec::with_capacity(self.headers.len());
        for width in &self.widths {
            offsets.push(at);
            at += width + COLUMN_GAP;
        }
        offsets.push(at);
        offsets
    }
}

/// The text after a line's timestamp, or the whole line when no timestamp is expected. `None`
/// when one is expected and missing.
fn after_timestamp(prefix: PrefixShape, line: &str) -> Option<&str> {
    if prefix.kind == PrefixKind::None {
        return Some(line);
    }
    let ts = prefix.measure_timestamp(line.as_bytes(), line.len());
    if ts == 0 {
        return None;
    }
    line.get(ts..)
}

/// Up to `max` leading fields of `text`.
fn fields(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while out.len() < max {
        let Some((field, after)) = next_field(rest) else {
            break;
        };
        out.push(field);
        rest = after.trim_start();
    }
    if !rest.is_empty() && out.len() == max {
        out.push(rest);
    }
    out
}

/// Splits off the first field: a `[...]` group (which may hold spaces), or a run of non-space.
fn next_field(text: &str) -> Option<(&str, &str)> {
    if text.is_empty() {
        return None;
    }
    let end = if text.starts_with('[') {
        text.find(']').map_or(text.len(), |i| i + 1)
    } else {
        text.find(char::is_whitespace).unwrap_or(text.len())
    };
    Some(text.split_at(end))
}

fn kind_of(field: &str) -> FieldKind {
    let bare = field.trim_start_matches('[').trim_end_matches([']', ':']);
    if LEVELS.contains(&bare) {
        return FieldKind::Level;
    }
    if field.len() > 2 && field.starts_with('[') && field.ends_with(']') {
        return FieldKind::Bracketed;
    }
    if !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()) {
        return FieldKind::Number;
    }
    if matches!(field, "-" | "|" | ":" | ">" | "--") {
        return FieldKind::Separator;
    }
    let bare = field.trim_end_matches(':');
    let dotted = bare.contains('.')
        && bare.len() > 2
        && bare.chars().any(|c| c.is_ascii_alphabetic())
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '$'))
        && !bare.starts_with('.')
        && !bare.ends_with('.');
    if dotted {
        return FieldKind::Dotted;
    }
    FieldKind::Other
}
