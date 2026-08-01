//! LanceDB predicate helpers.
//!
//! LanceDB's `only_if` / `delete` accept SQL-like strings; values coming from
//! user/LLM content must be escaped before interpolation.

use arrow_array::{Array, RecordBatch, StringArray};

pub fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

pub fn eq_predicate(column: &str, value: &str) -> String {
    format!("{} = '{}'", column, sql_escape(value))
}

/// Current time as RFC3339 — the timestamp format of `updated_at` /
/// `deleted_at` columns.
pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A nullable Utf8 column that may be absent in tables created before the
/// column was introduced.
pub(crate) fn optional_string_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Option<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
}

/// The value at row `i` of a column from `optional_string_column`, treating
/// missing columns, nulls and empty strings as `None`.
pub(crate) fn optional_string_at(column: Option<&StringArray>, i: usize) -> Option<String> {
    column.and_then(|c| {
        if c.is_null(i) || c.value(i).is_empty() {
            None
        } else {
            Some(c.value(i).to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_single_quote() {
        assert_eq!(sql_escape("l'article"), "l''article");
    }

    #[test]
    fn injection_neutralized() {
        let malicious = "x' OR id IS NOT NULL --";
        let pred = eq_predicate("id", malicious);
        assert_eq!(pred, "id = 'x'' OR id IS NOT NULL --'");
    }
}
