//! Splitting structured log lines into columns: timestamp, level, thread and so on, then the
//! message.
//!
//! The layout is guessed from a sample. A leading timestamp is recognised with the same
//! `PrefixShape` clustering uses; after it, each whitespace-separated field becomes a column for
//! as long as most sampled lines agree on what kind of field sits in that position. Everything
//! after the last agreed field is the message. A line that doesn't fit the layout (a stack
//! trace, a wrapped message) gets no columns at all.
//!
//! Lines that are mostly JSON objects get a column per top-level key instead, in the order the
//! keys are first seen, with a message-like key moved last.

use std::borrow::Cow;

use serde_json::{Map, Value};

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

/// JSON keys taken to hold the message, so that column goes last; compared ignoring case.
const MESSAGE_KEYS: [&str; 4] = ["message", "msg", "@message", "@m"];

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
enum Layout {
    /// An optional timestamp, then whitespace-separated fields of these kinds.
    Text {
        prefix: PrefixShape,
        fields: Vec<FieldKind>,
    },
    /// One JSON object per line; each header is a key.
    Json,
}

#[derive(Clone, Debug)]
pub struct ColumnShape {
    layout: Layout,
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
    if let Some(shape) = detect_json_columns(sample) {
        return Some(shape);
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

    Some(fit_widths(
        Layout::Text {
            prefix,
            fields: chosen,
        },
        headers,
        sample,
    ))
}

/// A column per JSON key when most sample lines are JSON objects.
fn detect_json_columns(sample: &[String]) -> Option<ColumnShape> {
    let needed = (sample.len() as f64 * AGREEMENT).ceil() as usize;
    let mut objects = 0;
    let mut keys: Vec<String> = Vec::new();
    for object in sample.iter().filter_map(|line| parse_object(line)) {
        objects += 1;
        for key in object.keys() {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    if objects < needed || keys.is_empty() {
        return None;
    }
    let message = keys
        .iter()
        .position(|k| MESSAGE_KEYS.iter().any(|m| k.eq_ignore_ascii_case(m)));
    if let Some(at) = message {
        let key = keys.remove(at);
        keys.push(key);
    }
    Some(fit_widths(Layout::Json, keys, sample))
}

/// The shape with each column but the last as wide as its header and widest sampled value.
fn fit_widths(layout: Layout, headers: Vec<String>, sample: &[String]) -> ColumnShape {
    let mut shape = ColumnShape {
        layout,
        widths: headers[..headers.len() - 1]
            .iter()
            .map(|h| h.chars().count().min(MAX_WIDTH))
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
    shape
}

impl ColumnShape {
    /// The line's shown columns, the message last, or `None` when it doesn't fit the layout.
    pub fn split<'a>(&self, line: &'a str) -> Option<Vec<Cow<'a, str>>> {
        let (prefix, fields) = match &self.layout {
            Layout::Text { prefix, fields } => (*prefix, fields),
            Layout::Json => return self.split_json(line),
        };
        let mut columns = Vec::with_capacity(self.headers.len());
        let rest = if prefix.kind == PrefixKind::None {
            line
        } else {
            let ts = prefix.measure_timestamp(line.as_bytes(), line.len());
            columns.push(Cow::Borrowed(line.get(..ts)?.trim_end()));
            after_timestamp(prefix, line)?
        };

        let mut rest = rest.trim_start();
        for kind in fields {
            let (field, after) = next_field(rest)?;
            if kind_of(field) != *kind {
                return None;
            }
            if *kind != FieldKind::Separator {
                columns.push(Cow::Borrowed(field));
            }
            rest = after.trim_start();
        }
        columns.push(Cow::Borrowed(rest));
        Some(columns)
    }

    /// A JSON line's value for each key, empty where it has none. `None` when the line isn't a
    /// JSON object or has a key the layout lacks, so nothing is hidden.
    fn split_json<'a>(&self, line: &str) -> Option<Vec<Cow<'a, str>>> {
        let mut object = parse_object(line)?;
        let columns = self
            .headers
            .iter()
            .map(|key| Cow::Owned(object.remove(key).map(json_text).unwrap_or_default()))
            .collect();
        object.is_empty().then_some(columns)
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
        out.push_str(split.last().map_or("", |v| v));
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

/// The line as a JSON object, keys in their written order.
fn parse_object(line: &str) -> Option<Map<String, Value>> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// A JSON value as shown in a column: strings unquoted, with line breaks and tabs flattened to
/// spaces so the value stays on its row; anything else as compact JSON.
fn json_text(value: Value) -> String {
    match value {
        Value::String(s) => s.replace(['\r', '\n', '\t'], " "),
        other => other.to_string(),
    }
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
