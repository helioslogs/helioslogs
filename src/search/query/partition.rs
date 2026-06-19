// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! `index:` partition filter: [`extract_partition_patterns`] collects `index:VALUE`
//! selectors and [`strip_partition_filters`] rewrites them to `Node::All` so the
//! catalog applies them as globs and the engine never sees a phantom column.

use super::ast::Node;

const PARTITION_FIELDS: &[&str] = &["index"];

/// Returns the raw value of every `index:` term in the AST. Wildcards
/// preserved; the caller applies glob matching.
pub fn extract_partition_patterns(node: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

fn walk(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::Term {
            field: Some(f),
            value,
            ..
        } if PARTITION_FIELDS.contains(&f.as_str()) => {
            out.push(value.clone());
        }
        Node::Term { .. } | Node::Range { .. } | Node::All => {}
        Node::And(c) | Node::Or(c) => {
            for x in c {
                walk(x, out);
            }
        }
        Node::Not(n) => walk(n, out),
    }
}

/// Returns a copy of the AST with every `index:` term replaced by
/// [`Node::All`] (match-everything).
pub fn strip_partition_filters(node: Node) -> Node {
    match node {
        Node::Term {
            field: Some(ref f), ..
        } if PARTITION_FIELDS.contains(&f.as_str()) => Node::All,
        n @ (Node::Term { .. } | Node::Range { .. } | Node::All) => n,
        Node::And(c) => Node::And(c.into_iter().map(strip_partition_filters).collect()),
        Node::Or(c) => Node::Or(c.into_iter().map(strip_partition_filters).collect()),
        Node::Not(n) => Node::Not(Box::new(strip_partition_filters(*n))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::query::parse;

    fn patterns(q: &str) -> Vec<String> {
        let node = parse(q).unwrap().unwrap();
        extract_partition_patterns(&node)
    }

    #[test]
    fn extracts_index_value() {
        assert_eq!(patterns("index:web error"), vec!["web".to_string()]);
    }

    #[test]
    fn extracts_multiple_across_or() {
        assert_eq!(
            patterns("index:a OR index:b"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn preserves_wildcards() {
        assert_eq!(patterns("index:web-*"), vec!["web-*".to_string()]);
    }

    #[test]
    fn source_is_not_a_partition_filter() {
        // `source:` is a normal schema field, left for the engine.
        assert!(patterns("source:nginx").is_empty());
    }

    #[test]
    fn strip_replaces_index_terms_with_all() {
        let node = parse("index:web error").unwrap().unwrap();
        let stripped = strip_partition_filters(node);
        // After stripping there are no index patterns left to extract.
        assert!(extract_partition_patterns(&stripped).is_empty());
        // The `error` term and an `All` survive inside the AND.
        match stripped {
            Node::And(conj) => {
                assert!(conj.iter().any(|n| matches!(n, Node::All)));
                assert!(conj
                    .iter()
                    .any(|n| matches!(n, Node::Term { value, .. } if value == "error")));
            }
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn strip_recurses_through_not() {
        let node = parse("-index:web").unwrap().unwrap();
        let stripped = strip_partition_filters(node);
        match stripped {
            Node::Not(inner) => assert!(matches!(*inner, Node::All)),
            other => panic!("expected Not(All), got {other:?}"),
        }
    }
}
