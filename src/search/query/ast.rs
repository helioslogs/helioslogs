// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Query AST — the expression tree the parser produces; variants are crate-public so
//! `query` submodules walk/rewrite it. Re-exported as [`crate::search::query::Node`].

use std::ops::Bound;

#[derive(Debug)]
pub enum Node {
    Term {
        field: Option<String>,
        value: String,
        quoted: bool,
    },
    Range {
        field: String,
        lo: Bound<String>,
        hi: Bound<String>,
    },
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// Match every document. Produced when `index:` partition filters are stripped.
    All,
}
