pub fn build_footer(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(key, action)| format!("{key}: {action}"))
        .collect::<Vec<_>>()
        .join(" | ")
}
