// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Byte-stream framing into one record per event: line mode (default) or
//! multiline mode (a `start_pattern` opens a record, other lines fold in). Pure.

use anyhow::{Context, Result};
use regex::Regex;

/// Compiled multiline rule. A line matching `start` begins a new record; lines
/// that don't match are folded into the current record (capped at `max_lines`).
pub struct Multiline {
    start: Regex,
    max_lines: usize,
}

impl Multiline {
    pub fn new(start_pattern: &str, max_lines: usize) -> Result<Self> {
        let start = Regex::new(start_pattern)
            .with_context(|| format!("invalid multiline start_pattern: {start_pattern}"))?;
        Ok(Self {
            start,
            max_lines: max_lines.max(1),
        })
    }
}

/// Split `text` into records. Blank lines drop in line mode; in multiline mode
/// they're kept as continuation so blanks inside a stack trace survive.
pub fn frame(text: &str, multiline: Option<&Multiline>) -> Vec<String> {
    match multiline {
        None => text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect(),
        Some(ml) => frame_multiline(text, ml),
    }
}

fn frame_multiline(text: &str, ml: &Multiline) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur: Option<(String, usize)> = None; // (record, line_count)

    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if ml.start.is_match(line) {
            // A new event begins — flush the one in progress.
            if let Some((rec, _)) = cur.take() {
                out.push(rec);
            }
            cur = Some((line.to_string(), 1));
        } else if let Some((rec, count)) = cur.as_mut() {
            // Continuation line — append unless we've hit the cap.
            if *count < ml.max_lines {
                rec.push('\n');
                rec.push_str(line);
                *count += 1;
            }
        } else if !line.trim().is_empty() {
            // Leading lines before any start match: emit standalone so nothing
            // is silently dropped.
            out.push(line.to_string());
        }
    }
    if let Some((rec, _)) = cur.take() {
        out.push(rec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_mode_handles_crlf_and_blanks() {
        let recs = frame("a\r\nb\n\n  \nc", None);
        assert_eq!(recs, vec!["a", "b", "c"]);
    }

    #[test]
    fn line_mode_no_trailing_newline() {
        assert_eq!(frame("only", None), vec!["only"]);
    }

    #[test]
    fn multiline_coalesces_stacktrace() {
        let ml = Multiline::new(r"^\d{4}-\d{2}-\d{2}", 100).unwrap();
        let input = "2026-05-31 ERROR boom\n  at foo()\n  at bar()\n2026-05-31 INFO ok";
        let recs = frame(input, Some(&ml));
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0], "2026-05-31 ERROR boom\n  at foo()\n  at bar()");
        assert_eq!(recs[1], "2026-05-31 INFO ok");
    }

    #[test]
    fn multiline_respects_max_lines() {
        let ml = Multiline::new(r"^START", 2).unwrap();
        let recs = frame("START\nl1\nl2\nl3", Some(&ml));
        // start + 1 continuation (cap 2), l2/l3 dropped past the cap.
        assert_eq!(recs, vec!["START\nl1"]);
    }

    #[test]
    fn multiline_leading_lines_not_dropped() {
        let ml = Multiline::new(r"^START", 100).unwrap();
        let recs = frame("preamble\nSTART\nmore", Some(&ml));
        assert_eq!(recs, vec!["preamble", "START\nmore"]);
    }
}
