//! Search utilities (fuzzy matching, etc.).

/// Case-insensitive subsequence match: every character in `query` appears
/// in `name` in order, ignoring case.
pub fn fuzzy_match(query: &str, name: &str) -> bool {
    let mut name_chars = name.chars().flat_map(|c| c.to_lowercase());
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        loop {
            match name_chars.next() {
                Some(nc) if nc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_exact() {
        assert!(fuzzy_match("parse", "parse"));
    }

    #[test]
    fn fuzzy_match_subsequence() {
        assert!(fuzzy_match("prs", "parse"));
        assert!(fuzzy_match("fmn", "file_manager_new"));
    }

    #[test]
    fn fuzzy_match_case_insensitive() {
        assert!(fuzzy_match("PRS", "parse"));
        assert!(fuzzy_match("prs", "PARSE"));
    }

    #[test]
    fn fuzzy_match_no_match() {
        assert!(!fuzzy_match("xyz", "parse"));
        assert!(!fuzzy_match("srp", "parse")); // wrong order
    }

    #[test]
    fn fuzzy_match_empty_query_matches_all() {
        assert!(fuzzy_match("", "anything"));
    }
}
