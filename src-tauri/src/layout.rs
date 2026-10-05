//! A dataset's row layout (#254): the ordered header list and where the
//! three mapped columns sit in it, stored as `datasets.layout`. Import
//! writes it for a file; a derived dataset gets the one its creator chose.
//! `courses.extra_columns` keys are positions in `headers`, and export puts
//! every row back together from the two.

use duckdb::OptionalExt as _;
use serde::{Deserialize, Serialize};

use crate::preflight::{ColumnMap, check_mapping};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub headers: Vec<String>,
    pub mapping: ColumnMap,
}

impl Layout {
    /// A layout is only usable when its mapping fits its header list.
    pub(crate) fn new(headers: Vec<String>, mapping: ColumnMap) -> Result<Self, String> {
        check_mapping(mapping, headers.len())?;
        Ok(Self { headers, mapping })
    }

    pub(crate) fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("serialize layout: {e}"))
    }

    /// The stored layout of a dataset: `None` when the dataset is unknown or
    /// predates stored layouts (imported before migration 0004). A stored
    /// layout that doesn't parse or check is an error, never `None`.
    pub(crate) fn read(
        conn: &duckdb::Connection,
        dataset_id: &str,
    ) -> Result<Option<Self>, String> {
        let json: Option<String> = conn
            .query_row(
                "SELECT layout::VARCHAR FROM datasets WHERE id = ?",
                [dataset_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("dataset {dataset_id}: {e}"))?
            .flatten();
        json.map(|j| {
            let Self { headers, mapping } =
                serde_json::from_str(&j).map_err(|e| format!("parse stored layout: {e}"))?;
            Self::new(headers, mapping).map_err(|e| format!("stored layout: {e}"))
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnMap, Layout};

    /// A layout round-trips through the JSON column, a dataset without one
    /// reads as `None`, and a stored layout whose mapping doesn't fit its
    /// headers is an error rather than a silent `None`.
    #[test]
    fn layout_round_trips_and_is_checked_on_read() -> Result<(), String> {
        let conn = duckdb::Connection::open_in_memory().map_err(|e| e.to_string())?;
        conn.execute_batch("CREATE TABLE datasets (id TEXT, layout JSON)")
            .map_err(|e| e.to_string())?;
        let layout = Layout::new(
            vec!["SUBJ".into(), "NUM".into(), "TITLE".into(), "SCHOOL".into()],
            ColumnMap {
                subject: 0,
                catalog: 1,
                title: 2,
            },
        )?;
        conn.execute(
            "INSERT INTO datasets VALUES ('a', ?), ('none', NULL), ('bad', '{\"headers\":[\"x\"],\"mapping\":{\"subject\":0,\"catalog\":1,\"title\":2}}')",
            [layout.to_json()?],
        )
        .map_err(|e| e.to_string())?;

        assert_eq!(Layout::read(&conn, "a")?, Some(layout));
        assert_eq!(Layout::read(&conn, "none")?, None);
        assert_eq!(Layout::read(&conn, "missing")?, None);
        assert!(Layout::read(&conn, "bad").is_err());
        Ok(())
    }
}
