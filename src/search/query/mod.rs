// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The query language: parse a query string into a [`Node`] AST the block engine
//! evaluates directly. Also exposes `index:` partition extraction/stripping and
//! highlight-term extraction.

mod ast;
mod highlights;
mod lex;
mod parse;
mod partition;

pub use ast::Node;
pub use highlights::extract_highlight_terms;
pub use parse::parse;
pub use partition::{extract_partition_patterns, strip_partition_filters};
