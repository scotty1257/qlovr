//! Number formatting for the status line and gutter.

/// `1234567` -> `"1,234,567"`, the .NET `"N0"` format the status line and gutter rely on.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// .NET `"N1"`: grouped integer part, one decimal place.
pub fn thousands_1dp(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    let whole = rounded.trunc().abs() as u64;
    let frac = ((rounded.abs() - whole as f64) * 10.0).round() as u64;
    let sign = if rounded < 0.0 { "-" } else { "" };
    format!("{sign}{}.{frac}", thousands(whole))
}
