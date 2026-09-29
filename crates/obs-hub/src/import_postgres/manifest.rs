//! `manifest.json` written by the export scripts: per-table row counts and Postgres column
//! types (`information_schema.columns.udt_name`), taken in the export's snapshot.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

const MANIFEST_FILE_NAME: &str = "manifest.json";

#[derive(Debug, Deserialize)]
pub struct ExportManifest {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub exported_at: Option<String>,
    #[serde(default)]
    pub snapshot: Option<String>,
    pub tables: Vec<ManifestTable>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestTable {
    pub name: String,
    pub row_count: u64,
    /// `json_agg` yields `null` for a table with no columns, hence `Option`.
    #[serde(default)]
    pub columns: Option<Vec<ManifestColumn>>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestColumn {
    pub name: String,
    /// Postgres `udt_name`, e.g. `uuid`, `timestamptz`, `int8`, `jsonb`, `bytea`.
    #[serde(rename = "type")]
    pub source_type: String,
}

impl ExportManifest {
    pub fn table(&self, name: &str) -> Option<&ManifestTable> {
        self.tables.iter().find(|table| table.name == name)
    }
}

impl ManifestTable {
    #[inline]
    pub fn columns(&self) -> &[ManifestColumn] {
        self.columns.as_deref().unwrap_or(&[])
    }
}

pub fn load_manifest(dump_dir: &Path) -> Result<ExportManifest> {
    let path = dump_dir.join(MANIFEST_FILE_NAME);
    let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;

    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_export_script_shape_with_spaced_keys_and_null_columns() {
        let text = r#"{"format" : "obs-postgres-export/v1", "snapshot" : "repeatable_read",
            "exported_at" : "2026-09-29T20:25:30.482Z",
            "tables" : [{"name" : "projects", "row_count" : 18,
                         "columns" : [{"name" : "project_id", "type" : "uuid"}]},
                        {"name" : "empty", "row_count" : 0, "columns" : null}]}"#;

        let manifest: ExportManifest = serde_json::from_str(text).unwrap();

        assert_eq!(manifest.table("projects").unwrap().row_count, 18);
        assert_eq!(
            manifest.table("projects").unwrap().columns()[0].source_type,
            "uuid"
        );
        assert!(manifest.table("empty").unwrap().columns().is_empty());
        assert!(manifest.table("missing").is_none());
    }
}
