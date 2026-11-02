use fastlogviewer_core::{TagDef, TAG_PALETTE};

#[test]
fn a_tag_saved_before_text_colours_keeps_its_background() {
    let tag: TagDef =
        serde_json::from_str(r#"{"Label": "err", "Pattern": "ERROR", "ColorIndex": 3}"#).unwrap();
    assert_eq!(Some(TAG_PALETTE[3]), tag.background());
    assert_eq!(None, tag.text_color());
}

#[test]
fn background_and_text_colours_are_independent() {
    let tag = TagDef {
        color_index: None,
        text_color_index: Some(1),
        ..Default::default()
    };
    assert_eq!(None, tag.background());
    assert_eq!(Some(TAG_PALETTE[1]), tag.text_color());
    // With no background the chip takes the text colour.
    assert_eq!(TAG_PALETTE[1], tag.color());

    let json = serde_json::to_string(&tag).unwrap();
    let back: TagDef = serde_json::from_str(&json).unwrap();
    assert_eq!(None, back.color_index);
    assert_eq!(Some(1), back.text_color_index);
}
