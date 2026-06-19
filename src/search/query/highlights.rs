// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Highlight-term extraction — the lowercase substrings the UI marks up in displayed
//! log text. Walks the AST for `message`/unscoped/dynamic-path terms; skips chip-
//! rendered fields, negated branches, ranges, and mid-term wildcards.

use super::ast::Node;
use super::lex::lex;
use super::parse::parse;

/// Returns deduplicated lowercased substrings to highlight in displayed text.
/// Errors quietly return empty — highlighting is a nicety, not correctness.
pub fn extract_highlight_terms(query_str: &str) -> Vec<String> {
    // Re-parse so this is callable independently of the build pipeline; parsing
    // is cheap relative to search.
    let trimmed = query_str.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return Vec::new();
    }
    let Ok(_) = lex(trimmed) else {
        return Vec::new();
    };
    let Ok(Some(node)) = parse(query_str) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    walk(&node, &mut out);
    out.sort();
    out.dedup();
    out
}

/// Fields rendered as their own chip, not highlighted inline (`source`, `index`).
const STRUCTURED_DISPLAY_FIELDS: &[&str] = &["source", "index"];

fn walk(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Term {
            field,
            value,
            quoted,
        } => {
            // Skip terms scoped to fields that have their own visual chip.
            if let Some(f) = field {
                if STRUCTURED_DISPLAY_FIELDS.contains(&f.as_str()) {
                    return;
                }
            }
            if *quoted {
                for word in value.split_whitespace() {
                    if let Some(s) = strip_wildcards(word) {
                        out.push(s);
                    }
                }
            } else if let Some(s) = strip_wildcards(value) {
                out.push(s);
            }
        }
        Node::Range { .. } | Node::All => {}
        Node::And(c) | Node::Or(c) => {
            for x in c {
                walk(x, out);
            }
        }
        Node::Not(_) => {}
    }
}

fn strip_wildcards(s: &str) -> Option<String> {
    let trimmed: String = s
        .trim_matches(|c: char| c == '*' || c == '?')
        .to_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('*') || trimmed.contains('?') {
        // Wildcards in the middle (e.g. `com?leted`) are too complex to
        // highlight cleanly — bail.
        return None;
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::extract_highlight_terms;

    #[test]
    fn empty_and_star_yield_nothing() {
        assert!(extract_highlight_terms("").is_empty());
        assert!(extract_highlight_terms("*").is_empty());
    }

    #[test]
    fn bare_term_lowercased() {
        assert_eq!(extract_highlight_terms("Error"), vec!["error".to_string()]);
    }

    #[test]
    fn dedup_and_sort() {
        assert_eq!(
            extract_highlight_terms("zebra apple apple"),
            vec!["apple".to_string(), "zebra".to_string()]
        );
    }

    #[test]
    fn quoted_phrase_splits_into_words() {
        assert_eq!(
            extract_highlight_terms(r#""foo bar""#),
            vec!["bar".to_string(), "foo".to_string()]
        );
    }

    #[test]
    fn chip_fields_skipped() {
        // `source:` and `index:` render as chips, not inline highlights.
        assert!(extract_highlight_terms("source:nginx").is_empty());
        assert!(extract_highlight_terms("index:web").is_empty());
    }

    #[test]
    fn dynamic_field_value_highlighted() {
        assert_eq!(
            extract_highlight_terms("status:500"),
            vec!["500".to_string()]
        );
    }

    #[test]
    fn negation_and_range_skipped() {
        assert!(extract_highlight_terms("-error").is_empty());
        assert!(extract_highlight_terms("latency:>100").is_empty());
    }

    #[test]
    fn trailing_wildcard_stripped_mid_wildcard_bails() {
        assert_eq!(extract_highlight_terms("err*"), vec!["err".to_string()]);
        assert!(extract_highlight_terms("com?leted").is_empty());
    }

    #[test]
    fn malformed_query_returns_empty() {
        // Highlighting is best-effort; a parse error yields no terms.
        assert!(extract_highlight_terms("(unclosed").is_empty());
    }
}
