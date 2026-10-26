use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::matcher::Matcher;

/// Tag highlight colours, as RGB. Index into this with `color_index % TAG_PALETTE.len()`.
pub const TAG_PALETTE: [[u8; 3]; 10] = [
    [0x2A, 0x66, 0xE0], // blue
    [0x1F, 0x9D, 0x55], // green
    [0xE0, 0x92, 0x00], // amber
    [0xC0, 0x30, 0x3D], // red
    [0x8B, 0x5C, 0xE0], // purple
    [0x0E, 0x9C, 0x9C], // teal
    [0xE0, 0x63, 0x1F], // orange
    [0xD6, 0x4B, 0x9E], // pink
    [0x2C, 0xA9, 0xD6], // cyan
    [0x7A, 0x82, 0x1F], // olive
];

/// A saved highlight/filter rule. Field names serialize in PascalCase so an existing `tags.json`
/// written by the C# build loads unchanged.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TagDef {
    pub id: Uuid,
    pub label: String,
    pub pattern: String,
    pub ignore_case: bool,
    pub color_index: usize,
    pub highlight_on: bool,
    pub filter_on: bool,

    #[serde(skip)]
    pub matcher: Option<Arc<Matcher>>,
}

impl Default for TagDef {
    fn default() -> Self {
        TagDef {
            id: Uuid::new_v4(),
            label: String::new(),
            pattern: String::new(),
            ignore_case: false,
            color_index: 0,
            highlight_on: true,
            filter_on: false,
            matcher: None,
        }
    }
}

impl TagDef {
    pub fn rebuild_matcher(&mut self) {
        self.matcher = Some(Arc::new(Matcher::new(&self.pattern, true, self.ignore_case)));
    }

    pub fn color(&self) -> [u8; 3] {
        TAG_PALETTE[self.color_index % TAG_PALETTE.len()]
    }
}

fn store_dir() -> Option<PathBuf> {
    // On Windows this is %APPDATA%, the same folder the C# build used.
    dirs::config_dir().map(|d| d.join("FastLogViewer"))
}

fn store_path() -> Option<PathBuf> {
    store_dir().map(|d| d.join("tags.json"))
}

#[derive(Default)]
pub struct TagStore {
    pub tags: Vec<TagDef>,
}

impl TagStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(&mut self) {
        self.tags.clear();
        // Corrupt or unreadable -> start empty, never crash on startup.
        let Some(path) = store_path() else { return };
        let Ok(json) = fs::read_to_string(path) else {
            return;
        };
        let Ok(loaded) = serde_json::from_str::<Vec<TagDef>>(&json) else {
            return;
        };
        for mut tag in loaded {
            tag.rebuild_matcher();
            self.tags.push(tag);
        }
    }

    /// Best-effort persistence; a failed save must not crash the app.
    pub fn save(&self) {
        let Some(dir) = store_dir() else { return };
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.tags) {
            let _ = fs::write(dir.join("tags.json"), json);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        label: &str,
        pattern: &str,
        ignore_case: bool,
        highlight_on: bool,
        filter_on: bool,
        color_index: Option<usize>,
    ) -> usize {
        let mut tag = TagDef {
            label: label.to_string(),
            pattern: pattern.to_string(),
            ignore_case,
            highlight_on,
            filter_on,
            color_index: color_index.unwrap_or(self.tags.len() % TAG_PALETTE.len()),
            ..Default::default()
        };
        tag.rebuild_matcher();
        self.tags.push(tag);
        self.save();
        self.tags.len() - 1
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.tags.len() {
            self.tags.remove(index);
            self.save();
        }
    }
}
