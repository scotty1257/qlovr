mod common;

use std::fmt::Write as _;
use std::sync::Arc;

use common::write_temp;
use fastlogviewer_core::{CancelToken, FilterIndex, LogFile, SlotFilterIndex, SlotQuery, TemplateIndex};

#[test]
fn filters_lines_of_a_template_by_a_slot_predicate() {
    let mut sb = String::new();
    let latencies = [12, 8391, 40, 3004, 7];
    for (i, latency) in latencies.iter().enumerate() {
        writeln!(sb, "GET /api/u/{i} 200 in {latency}ms").unwrap();
    }

    let path = write_temp(&sb);
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let idx = TemplateIndex::default();
        idx.build(&f, None, &CancelToken::none());

        let t = idx.snapshot_sorted().remove(0);
        assert_eq!("GET /api/u/<*> 200 in <*>ms", t.render());

        let q = SlotQuery::parse("slot2 > 2000", t.slot_count());
        assert!(q.error.is_none());

        let slots = SlotFilterIndex::new(t, Arc::new(q), idx.prefix(), f.length());
        slots.build(&f, &CancelToken::none());

        assert!(slots.is_complete());
        assert_eq!(2, slots.count());
        assert_eq!(1, slots.get(0)); // 8391ms
        assert_eq!(3, slots.get(1)); // 3004ms
        assert_eq!(0, slots.index_at_or_after(0));
        assert_eq!(1, slots.index_at_or_after(2));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn an_invalid_query_yields_an_empty_complete_index() {
    let path = write_temp("GET /a 200 in 5ms\nGET /b 200 in 6ms\n");
    {
        let f = LogFile::open(&path).unwrap();
        f.build_index(&CancelToken::none());
        let idx = TemplateIndex::default();
        idx.build(&f, None, &CancelToken::none());

        let t = idx.snapshot_sorted().remove(0);
        let bad = SlotQuery::parse("slot9 > 1", t.slot_count());
        assert!(bad.error.is_some());

        let slots = SlotFilterIndex::new(t, Arc::new(bad), idx.prefix(), f.length());
        slots.build(&f, &CancelToken::none());

        assert!(slots.is_complete());
        assert_eq!(0, slots.count());
    }
    std::fs::remove_file(path).unwrap();
}
