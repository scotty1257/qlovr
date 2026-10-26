use fastlogviewer_core::{TagDef, TagStore};

fn tag(label: &str, level: bool, filter_on: bool) -> TagDef {
    TagDef {
        label: label.to_string(),
        pattern: label.to_string(),
        level,
        filter_on,
        ..Default::default()
    }
}

fn store(tags: Vec<TagDef>) -> TagStore {
    TagStore { tags }
}

fn labels(store: &TagStore) -> Vec<String> {
    store
        .filtering_ids()
        .iter()
        .map(|id| store.tags.iter().find(|t| t.id == *id).unwrap().label.clone())
        .collect()
}

#[test]
fn lowest_level_pulls_in_every_level_to_its_right() {
    let s = store(vec![
        tag("INFO", true, true),
        tag("WARNING", true, false),
        tag("ERROR", true, false),
    ]);
    assert_eq!(vec!["INFO", "WARNING", "ERROR"], labels(&s));
}

#[test]
fn highest_level_filters_only_itself() {
    let s = store(vec![
        tag("INFO", true, false),
        tag("WARNING", true, false),
        tag("ERROR", true, true),
    ]);
    assert_eq!(vec!["ERROR"], labels(&s));
}

#[test]
fn non_level_tags_keep_their_own_filter_switch() {
    let s = store(vec![
        tag("INFO", true, false),
        tag("SomeFile", false, false),
        tag("WARNING", true, true),
        tag("Timeout", false, true),
        tag("ERROR", true, false),
    ]);
    assert_eq!(vec!["WARNING", "Timeout", "ERROR"], labels(&s));
}

#[test]
fn nothing_filtered_when_no_switch_is_on() {
    let s = store(vec![tag("INFO", true, false), tag("x", false, false)]);
    assert!(s.filtering_ids().is_empty());
}

#[test]
fn move_tag_swaps_with_its_neighbour() {
    let mut s = store(vec![tag("A", true, false), tag("B", true, false), tag("C", true, false)]);
    assert_eq!(Some(2), s.move_tag(1, 1));
    let order: Vec<_> = s.tags.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(vec!["A", "C", "B"], order);
    assert_eq!(None, s.move_tag(2, 1));
    assert_eq!(None, s.move_tag(0, -1));
}
