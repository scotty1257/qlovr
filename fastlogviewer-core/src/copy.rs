//! Turning lines into clipboard text: the "from-to" range a user types, and joining lines without
//! letting a select-all on a multi-GB file build a multi-GB string.

use crate::log_file::LogFile;

/// Most bytes one copy puts on the clipboard.
pub const COPY_LIMIT_BYTES: usize = 64 << 20;
/// Longest single line a copy takes, far past what the view decodes for drawing.
const COPY_LINE_BYTES: usize = 1 << 20;

/// Lines joined for the clipboard.
pub struct Joined {
    pub text: String,
    /// How many lines made it in.
    pub lines: usize,
    /// The byte limit cut the copy short.
    pub truncated: bool,
}

/// Decodes a line for copying: tabs expanded the way the view shows them, no line terminator.
pub fn copy_text_of(file: &LogFile, line: usize) -> String {
    file.get_line(line, COPY_LINE_BYTES)
}

/// Joins `lines` with `\n`, stopping before the line that would take the text past `limit` bytes.
pub fn join_lines(file: &LogFile, lines: impl Iterator<Item = usize>, limit: usize) -> Joined {
    join_texts(lines.map(|line| copy_text_of(file, line)), limit)
}

/// Joins already-decoded pieces with `\n` under the same byte limit as `join_lines`. Lazy, so
/// nothing past the limit is decoded.
pub fn join_texts(texts: impl Iterator<Item = String>, limit: usize) -> Joined {
    let mut joined = Joined {
        text: String::new(),
        lines: 0,
        truncated: false,
    };
    for text in texts {
        let separator = usize::from(joined.lines > 0);
        if joined.text.len() + separator + text.len() > limit {
            joined.truncated = true;
            break;
        }
        if separator == 1 {
            joined.text.push('\n');
        }
        joined.text.push_str(&text);
        joined.lines += 1;
    }
    joined
}

/// Parses a 1-based, inclusive line range such as `10-20`, `10..20` or a single `10`, and returns
/// it 0-based. Reversed ends are swapped and an end past the last line is clamped to it; a start
/// past the last line is an error.
pub fn parse_line_range(text: &str, line_count: usize) -> Result<(usize, usize), String> {
    let number = |part: &str| -> Result<usize, String> {
        let digits: String = part.trim().chars().filter(|c| *c != ',').collect();
        digits
            .parse::<usize>()
            .map_err(|_| format!("\"{}\" is not a line number", part.trim()))
    };

    let parts: Vec<&str> = text.split("..").flat_map(|p| p.split('-')).collect();
    let (first, last) = match parts.as_slice() {
        [one] => (number(one)?, number(one)?),
        [from, to] => (number(from)?, number(to)?),
        _ => return Err("Use from-to, for example 120-180".to_string()),
    };
    let (first, last) = (first.min(last), first.max(last));

    if first == 0 {
        return Err("Lines start at 1".to_string());
    }
    if first > line_count {
        return Err(format!("The file has {line_count} lines"));
    }
    Ok((first - 1, last.min(line_count) - 1))
}
