// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The write path: [`parse`] (pure `bytes → Vec<Value>`) feeds [`ingest`]
//! (JSON → block-engine rows), with the [`tokenizer`] splitting text fields.

pub mod ingest;
pub mod parse;
pub mod tokenizer;
