use fancy_regex::{Regex, RegexBuilder};

/// Immutable search predicate: literal or regex, case sensitive or not.
///
/// Highlight ranges are reported in **character** positions, not bytes. The viewer draws with a
/// monospace font and multiplies a column by the cell width, so a byte offset would smear every
/// highlight on any line containing non-ASCII text.
pub struct Matcher {
    pub pattern: String,
    pub use_regex: bool,
    pub ignore_case: bool,
    /// Non-`None` when the regex failed to compile.
    pub error: Option<String>,
    /// Non-`None` when the fast byte-scan path applies.
    pub literal_bytes: Option<Vec<u8>>,

    rx: Option<Regex>,
    pattern_chars: Vec<char>,
}

impl Matcher {
    pub fn new(pattern: &str, use_regex: bool, ignore_case: bool) -> Self {
        let mut m = Matcher {
            pattern: pattern.to_string(),
            use_regex,
            ignore_case,
            error: None,
            literal_bytes: None,
            rx: None,
            pattern_chars: pattern.chars().collect(),
        };

        if m.is_empty() {
            return m;
        }

        if use_regex {
            // `fancy-regex` keeps .NET-style lookaround and backreferences working, which the
            // linear-time `regex` crate alone would reject.
            match RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .build()
            {
                Ok(rx) => m.rx = Some(rx),
                Err(e) => m.error = Some(e.to_string()),
            }
            return m;
        }

        // Literal + case sensitive + plain ASCII (no tab, we expand those) -> raw byte scan.
        if !ignore_case {
            let scannable = pattern.chars().all(|c| c.is_ascii() && c != '\t');
            if scannable {
                m.literal_bytes = Some(pattern.as_bytes().to_vec());
            }
        }
        m
    }

    pub fn is_empty(&self) -> bool {
        self.pattern.is_empty()
    }

    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }

    pub fn is_match(&self, line: &str) -> bool {
        if self.is_empty() || !self.is_valid() {
            return false;
        }
        if let Some(rx) = &self.rx {
            return rx.is_match(line).unwrap_or(false);
        }
        if !self.ignore_case {
            return line.contains(&self.pattern);
        }
        let hay: Vec<char> = line.chars().collect();
        find_in_chars(&hay, &self.pattern_chars, 0, true).is_some()
    }

    /// Appends (start, length) pairs — in character positions — of every match in `line`.
    pub fn append_ranges(&self, line: &str, ranges: &mut Vec<i32>) {
        if self.is_empty() || !self.is_valid() || line.is_empty() {
            return;
        }

        if let Some(rx) = &self.rx {
            let bounds = char_bounds(line);
            let mut pos = 0usize;
            let mut guard = 0;
            while guard < 4096 {
                guard += 1;
                let found = match rx.find_from_pos(line, pos) {
                    Ok(Some(m)) => m,
                    _ => break,
                };
                let (start, end) = (found.start(), found.end());
                if end > start {
                    let c0 = char_index(&bounds, start);
                    let c1 = char_index(&bounds, end);
                    ranges.push(c0);
                    ranges.push(c1 - c0);
                }
                // A zero-width match must still make progress, and it has to land on a char
                // boundary or `find_from_pos` will panic on the next round.
                let next = if end > start {
                    end
                } else {
                    next_boundary(line, start)
                };
                if next > line.len() || next <= pos && end == start && next == pos {
                    break;
                }
                pos = next;
            }
            return;
        }

        let hay: Vec<char> = line.chars().collect();
        let needle = &self.pattern_chars;
        if needle.is_empty() || needle.len() > hay.len() {
            return;
        }
        let mut i = 0usize;
        while i + needle.len() <= hay.len() {
            match find_in_chars(&hay, needle, i, self.ignore_case) {
                Some(p) => {
                    ranges.push(p as i32);
                    ranges.push(needle.len() as i32);
                    i = p + needle.len();
                }
                None => break,
            }
        }
    }
}

fn chars_eq(a: char, b: char, ignore_case: bool) -> bool {
    if a == b {
        return true;
    }
    ignore_case && a.to_lowercase().eq(b.to_lowercase())
}

fn find_in_chars(hay: &[char], needle: &[char], from: usize, ignore_case: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    (from..=last).find(|&start| {
        needle
            .iter()
            .enumerate()
            .all(|(k, &c)| chars_eq(hay[start + k], c, ignore_case))
    })
}

/// Byte offset of every character in `line`, so a regex's byte range can be reported as columns.
fn char_bounds(line: &str) -> Vec<usize> {
    line.char_indices().map(|(i, _)| i).collect()
}

fn char_index(bounds: &[usize], byte: usize) -> i32 {
    match bounds.binary_search(&byte) {
        Ok(i) => i as i32,
        Err(i) => i as i32, // a byte inside a char counts as that char's column
    }
}

fn next_boundary(line: &str, from: usize) -> usize {
    let mut i = from + 1;
    while i < line.len() && !line.is_char_boundary(i) {
        i += 1;
    }
    i
}
