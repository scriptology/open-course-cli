//! LanceDB predicate helpers.
//!
//! LanceDB's `only_if` / `delete` accept SQL-like strings; values coming from
//! user/LLM content must be escaped before interpolation.

pub fn sql_escape(value: &str) -> String {
    value.replace('\'', "''")
}

pub fn eq_predicate(column: &str, value: &str) -> String {
    format!("{} = '{}'", column, sql_escape(value))
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
