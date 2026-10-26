mod common;

use std::fmt::Write as _;
use std::sync::Arc;

use common::write_temp;
use fastlogviewer_core::{
    CancelToken, FilterIndex, LogFile, MatchIndex, Matcher, PrefixKind, TemplateIndex,
};

#[test]
fn rare_threshold_matches_spec() {
    for (clustered, expected) in [(0, 5), (100_000, 5), (600_000, 6), (4_102_338, 41)] {
        assert_eq!(expected, TemplateIndex::rare_threshold_for(clustered));
    }
}

#[test]
fn clusters_a_file_end_to_end_with_prefix_stripping_and_rarity() {
    let mut sb = String::new();
    for i in 0..5000 {
        writeln!(
            sb,
            "2026-08-07T14:02:31.447Z INFO GET /api/u/{} 200 in {}ms",
            i,
            i % 90 + 1
        )
        .unwrap();
    }
    for i in 0..40 {
        writeln!(
            sb,
            "2026-08-07T14:02:32.000Z WARN retrying u/{i} attempt 2/5"
        )
        .unwrap();
    }
    // Three occurrences on different shards, so the shard id becomes a slot — one occurrence would
    // (correctly) leave it literal, since nothing ever disagrees with it.
    for shard in [4, 9, 17] {
        writeln!(
            sb,
            "2026-08-07T14:02:33.000Z ERROR checksum mismatch on shard {shard}"
        )
        .unwrap();
    }

    let path = write_temp(&sb);
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let idx = TemplateIndex::default();
        idx.build(&f, None, &CancelToken::none());

        assert!(idx.is_complete());
        assert_eq!(5043, idx.clustered_lines());
        assert_eq!(PrefixKind::Iso, idx.prefix().kind);

        let t = idx.snapshot_sorted();
        assert_eq!("GET /api/u/<*> 200 in <*>ms", t[0].render());
        assert_eq!(5000, t[0].count());
        assert!(!idx.is_rare(&t[0]));

        let rarest = t.last().unwrap();
        assert_eq!("checksum mismatch on shard <*>", rarest.render());
        assert_eq!(3, rarest.count());
        assert!(idx.is_rare(rarest));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn respects_the_active_filter_scope() {
    let path = write_temp("alpha 1\nbeta 1\nalpha 2\nbeta 2\nalpha 3\n");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let scope = MatchIndex::new(Arc::new(Matcher::new("alpha", false, false)), f.length());
        scope.build(&f, &CancelToken::none());

        let idx = TemplateIndex::default();
        idx.build(&f, Some(&scope as &dyn FilterIndex), &CancelToken::none());

        assert_eq!(3, idx.clustered_lines());
        let t = idx.snapshot_sorted();
        assert_eq!(1, t.len());
        assert_eq!("alpha <*>", t[0].render());
        assert_eq!(0, t[0].get(0));
        assert_eq!(2, t[0].get(1));
        assert_eq!(4, t[0].get(2));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cancellation_stops_the_build_without_completing() {
    let mut sb = String::new();
    for i in 0..200_000 {
        writeln!(sb, "line {i}").unwrap();
    }
    let path = write_temp(&sb);

    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let ct = CancelToken::new();
        ct.cancel();
        let idx = TemplateIndex::default();
        idx.build(&f, None, &ct);
        assert!(!idx.is_complete());
    }
    std::fs::remove_file(path).unwrap();
}
