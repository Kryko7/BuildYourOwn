//! The stage catalogs the testers/site generate, read for `byo status`' section bars.
//!
//! Only the fields `byo` needs are modelled; everything else in the file is ignored, and a
//! missing catalog is never fatal (the status view falls back to whatever the database
//! knows).

use serde::Deserialize;
use std::path::Path;

/// One track's stage catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    /// `shell` or `kafka`.
    #[serde(default)]
    pub track: String,
    /// Sections (A, B, …), each listing its stage numbers.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Every stage.
    #[serde(default)]
    pub stages: Vec<StageInfo>,
}

/// A group of stages.
#[derive(Debug, Clone, Deserialize)]
pub struct Section {
    /// Short id, e.g. `A`.
    #[serde(default)]
    pub id: String,
    /// Human title.
    #[serde(default)]
    pub title: String,
    /// Stage numbers in this section.
    #[serde(default)]
    pub stages: Vec<u32>,
}

/// One stage of a catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct StageInfo {
    /// Stage number, 1-based.
    pub number: u32,
    /// Stage name.
    #[serde(default)]
    pub name: String,
    /// True when the stage goes beyond the core track.
    #[serde(default)]
    pub ext: bool,
}

impl Catalog {
    /// The name of a stage, if the catalog knows it.
    pub fn name_of(&self, n: u32) -> Option<&str> {
        self.stages
            .iter()
            .find(|s| s.number == n)
            .map(|s| s.name.as_str())
    }

    /// Every stage number, ascending.
    pub fn numbers(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.stages.iter().map(|s| s.number).collect();
        v.sort_unstable();
        v
    }
}

/// Read a catalog, returning `None` when it is missing or unreadable.
pub fn load(path: &Path) -> Option<Catalog> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "track": "shell", "generatedAt": "now",
      "sections": [{"id":"A","title":"Basics","stages":[1,2]}],
      "stages": [
        {"number":1,"slug":"prompt","name":"Prompt","ext":false,"file":"01.yaml","hints":[],"tests":[]},
        {"number":2,"slug":"bad","name":"Invalid command","ext":true,"file":"02.yaml","hints":[],"tests":[]}
      ],
      "totals": {"stages": 2}
    }"#;

    #[test]
    fn parses_and_ignores_extra_fields() {
        let c: Catalog = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(c.track, "shell");
        assert_eq!(c.sections[0].title, "Basics");
        assert_eq!(c.name_of(2), Some("Invalid command"));
        assert_eq!(c.numbers(), vec![1, 2]);
        assert!(c.stages[1].ext);
    }

    #[test]
    fn missing_file_is_none() {
        assert!(load(Path::new("/definitely/not/here.json")).is_none());
    }
}
