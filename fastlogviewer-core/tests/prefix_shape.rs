use fastlogviewer_core::{PrefixKind, PrefixShape};

fn lines(lines: &[&str]) -> Vec<Vec<u8>> {
    lines.iter().map(|s| s.as_bytes().to_vec()).collect()
}

fn stripped(shape: PrefixShape, line: &str) -> String {
    let b = line.as_bytes();
    let start = shape.prefix_length(b, b.len());
    String::from_utf8_lossy(&b[start..]).into_owned()
}

#[test]
fn detects_iso_with_level_token() {
    let s = PrefixShape::sniff(&lines(&[
        "2026-08-07T14:02:31.447Z INFO  GET /api/u/91 200 in 12ms",
        "2026-08-07T14:02:32.001Z ERROR checksum mismatch on shard 4",
    ]));
    assert_eq!(PrefixKind::Iso, s.kind);
    assert_eq!(
        "GET /api/u/91 200 in 12ms",
        stripped(s, "2026-08-07T14:02:31.447Z INFO  GET /api/u/91 200 in 12ms")
    );
}

#[test]
fn detects_syslog_bracket_and_bare_time() {
    assert_eq!(
        PrefixKind::Syslog,
        PrefixShape::sniff(&lines(&[
            "Aug  7 14:02:31 host daemon started",
            "Aug  7 14:02:32 host daemon ready",
        ]))
        .kind
    );

    assert_eq!(
        PrefixKind::Bracket,
        PrefixShape::sniff(&lines(&[
            "[2026-08-07 14:02:31] pool exhausted",
            "[2026-08-07 14:02:33] pool recovered",
        ]))
        .kind
    );

    assert_eq!(
        PrefixKind::BareTime,
        PrefixShape::sniff(&lines(&[
            "14:02:31.447 cache hit u/91",
            "14:02:31.900 cache hit u/92",
        ]))
        .kind
    );
}

#[test]
fn requires_eighty_percent_agreement() {
    // 8 of 10 match -> accepted
    let mostly = lines(&[
        "2026-08-07T14:00:00Z a",
        "2026-08-07T14:00:01Z b",
        "2026-08-07T14:00:02Z c",
        "2026-08-07T14:00:03Z d",
        "2026-08-07T14:00:04Z e",
        "2026-08-07T14:00:05Z f",
        "2026-08-07T14:00:06Z g",
        "2026-08-07T14:00:07Z h",
        "no timestamp here",
        "nor here",
    ]);
    assert_eq!(PrefixKind::Iso, PrefixShape::sniff(&mostly).kind);

    // 7 of 10 -> rejected
    let not_enough = lines(&[
        "2026-08-07T14:00:00Z a",
        "2026-08-07T14:00:01Z b",
        "2026-08-07T14:00:02Z c",
        "2026-08-07T14:00:03Z d",
        "2026-08-07T14:00:04Z e",
        "2026-08-07T14:00:05Z f",
        "2026-08-07T14:00:06Z g",
        "no timestamp here",
        "nor here",
        "nor this one",
    ]);
    assert_eq!(PrefixKind::None, PrefixShape::sniff(&not_enough).kind);
}

#[test]
fn no_prefix_strips_nothing() {
    let s = PrefixShape::sniff(&lines(&["plain message one", "plain message two"]));
    assert_eq!(PrefixKind::None, s.kind);
    assert_eq!("plain message one", stripped(s, "plain message one"));
}

#[test]
fn exposes_timestamp_span_separately_from_full_prefix() {
    let s = PrefixShape::sniff(&lines(&[
        "2026-08-07T14:02:31.447Z INFO msg",
        "2026-08-07T14:02:32.447Z WARN msg",
    ]));
    let b = b"2026-08-07T14:02:31.447Z INFO msg";
    assert_eq!(24, s.measure_timestamp(b, b.len())); // just the timestamp
    assert_eq!(30, s.prefix_length(b, b.len())); // timestamp + " INFO "
}
