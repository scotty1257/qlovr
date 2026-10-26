#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

struct Pred {
    slot: usize, // zero-based after parse
    op: Op,
    text: String,
    number: f64,
    is_numeric: bool,
}

/// Predicates over a template's variable slots, e.g. "slot2 > 2000 AND slot1 = warn". Slots are
/// 1-based over the template's slot positions, not its token positions. AND binds tighter than OR
/// and there are no parentheses, so the whole thing is an OR of AND-groups.
///
/// Only meaningful while zoomed into a template — outside that context the search box keeps its
/// existing behaviour.
pub struct SlotQuery {
    or_groups: Vec<Vec<Pred>>,
    pub error: Option<String>,
}

/// `double.TryParse(value, NumberStyles.Float, InvariantCulture)` — the surrounding whitespace that
/// `NumberStyles.Float` tolerates is why this trims before parsing.
fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

fn starts_with_slot(token: &str) -> bool {
    token.len() >= 4 && token[..4].eq_ignore_ascii_case("slot")
}

impl SlotQuery {
    /// "slot" followed by a digit — the disambiguator that tells the search box this is a slot
    /// predicate rather than an ordinary search.
    pub fn looks_like_slot_query(text: &str) -> bool {
        let s = text.trim_start();
        let chars: Vec<char> = s.chars().take(5).collect();
        if chars.len() < 5 {
            return false;
        }
        let head: String = chars[..4].iter().collect();
        head.eq_ignore_ascii_case("slot") && chars[4].is_ascii_digit()
    }

    pub fn parse(text: &str, slot_count: usize) -> SlotQuery {
        let mut query = SlotQuery {
            or_groups: Vec::new(),
            error: None,
        };

        let tokens = scan(text);
        if tokens.is_empty() {
            query.error = Some("empty slot filter".to_string());
            return query;
        }

        let mut current: Vec<Pred> = Vec::new();
        let mut i = 0usize;
        loop {
            match parse_pred(&tokens, &mut i, slot_count) {
                Ok(pred) => current.push(pred),
                Err(e) => {
                    query.error = Some(e);
                    return query;
                }
            }

            if i >= tokens.len() {
                break;
            }

            let op = &tokens[i];
            if op.eq_ignore_ascii_case("AND") {
                i += 1;
                continue;
            }
            if op.eq_ignore_ascii_case("OR") {
                i += 1;
                query.or_groups.push(std::mem::take(&mut current));
                continue;
            }
            query.error = Some(format!("expected AND or OR, found '{op}'"));
            return query;
        }

        query.or_groups.push(current);
        query
    }

    pub fn evaluate(&self, slot_values: &[Option<String>]) -> bool {
        if self.error.is_some() {
            return false;
        }
        self.or_groups
            .iter()
            .any(|group| group.iter().all(|p| matches_pred(p, slot_values)))
    }
}

fn parse_pred(tokens: &[String], i: &mut usize, slot_count: usize) -> Result<Pred, String> {
    if *i + 2 >= tokens.len() {
        return Err("incomplete slot predicate".to_string());
    }

    let slot_token = &tokens[*i];
    *i += 1;
    if !starts_with_slot(slot_token) {
        return Err(format!("expected slotN, found '{slot_token}'"));
    }

    let slot: usize = match slot_token[4..].trim().parse() {
        Ok(n) => n,
        Err(_) => return Err(format!("expected a slot number in '{slot_token}'")),
    };
    if slot < 1 || slot > slot_count {
        return Err(format!(
            "slot {slot} is out of range (this template has {slot_count})"
        ));
    }

    let op_token = &tokens[*i];
    *i += 1;
    let op = match op_token.as_str() {
        "=" | "==" => Op::Eq,
        "!=" | "<>" => Op::Ne,
        ">" => Op::Gt,
        ">=" => Op::Ge,
        "<" => Op::Lt,
        "<=" => Op::Le,
        _ => return Err(format!("unknown operator '{op_token}'")),
    };

    let value = tokens[*i].clone();
    *i += 1;
    let number = parse_number(&value);

    if number.is_none() && op != Op::Eq && op != Op::Ne {
        return Err(format!(
            "'{value}' is not numeric; <, <=, >, >= need a numeric value"
        ));
    }

    Ok(Pred {
        slot: slot - 1,
        op,
        text: value,
        number: number.unwrap_or(0.0),
        is_numeric: number.is_some(),
    })
}

/// Splits on whitespace, keeps quoted strings whole, and lifts operators out of unspaced input so
/// "slot1>5" scans the same as "slot1 > 5".
fn scan(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    if text.is_empty() {
        return tokens;
    }

    let chars: Vec<char> = text.chars().collect();
    let mut buf = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            flush(&mut tokens, &mut buf);
            i += 1;
            continue;
        }

        if c == '"' {
            flush(&mut tokens, &mut buf);
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            tokens.push(std::mem::take(&mut buf)); // an empty quoted string is still a value
            continue;
        }

        if c == '=' || c == '!' || c == '<' || c == '>' {
            flush(&mut tokens, &mut buf);
            buf.push(c);
            i += 1;
            if i < chars.len() && (chars[i] == '=' || chars[i] == '>') {
                buf.push(chars[i]);
                i += 1;
            }
            flush(&mut tokens, &mut buf);
            continue;
        }

        buf.push(c);
        i += 1;
    }
    flush(&mut tokens, &mut buf);
    tokens
}

fn flush(tokens: &mut Vec<String>, buf: &mut String) {
    if buf.is_empty() {
        return;
    }
    tokens.push(std::mem::take(buf));
}

fn matches_pred(p: &Pred, slot_values: &[Option<String>]) -> bool {
    if p.slot >= slot_values.len() {
        return false;
    }
    let Some(value) = slot_values[p.slot].as_deref() else {
        return false;
    };

    if p.is_numeric {
        if let Some(got) = parse_number(value) {
            return match p.op {
                Op::Eq => got == p.number,
                Op::Ne => got != p.number,
                Op::Gt => got > p.number,
                Op::Ge => got >= p.number,
                Op::Lt => got < p.number,
                Op::Le => got <= p.number,
            };
        }
    }

    match p.op {
        Op::Eq => value == p.text,
        Op::Ne => value != p.text,
        _ => false, // ordering against a non-numeric line value
    }
}
