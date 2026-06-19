// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Recursive-descent parser: token stream → `Node` AST. Precedence is OR over
//! (implicit/explicit) AND over NOT over atoms; words may carry `field:value`.

use anyhow::{bail, Result};
use std::ops::Bound;

use super::ast::Node;
use super::lex::{lex, Tok};

/// Parse-only entry point. Returns `None` for empty/`*` queries (match-all).
pub fn parse(query_str: &str) -> Result<Option<Node>> {
    let trimmed = query_str.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return Ok(None);
    }
    let tokens = lex(trimmed)?;
    let mut parser = Parser { tokens, pos: 0 };
    let node = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        bail!("unexpected token at position {}", parser.pos);
    }
    Ok(Some(node))
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<Node> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Node> {
        let first = self.parse_and()?;
        let mut alts = vec![first];
        while matches!(self.peek(), Some(Tok::Or)) {
            self.bump();
            alts.push(self.parse_and()?);
        }
        if alts.len() == 1 {
            Ok(alts.pop().unwrap())
        } else {
            Ok(Node::Or(alts))
        }
    }

    fn parse_and(&mut self) -> Result<Node> {
        let mut conj = vec![self.parse_not()?];
        loop {
            match self.peek() {
                None | Some(Tok::RParen) | Some(Tok::Or) => break,
                Some(Tok::And) => {
                    self.bump();
                    conj.push(self.parse_not()?);
                }
                _ => {
                    conj.push(self.parse_not()?);
                }
            }
        }
        if conj.len() == 1 {
            Ok(conj.pop().unwrap())
        } else {
            Ok(Node::And(conj))
        }
    }

    fn parse_not(&mut self) -> Result<Node> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.bump();
            let inner = self.parse_atom()?;
            Ok(Node::Not(Box::new(inner)))
        } else {
            self.parse_atom()
        }
    }

    fn parse_atom(&mut self) -> Result<Node> {
        match self.bump() {
            Some(Tok::LParen) => {
                let n = self.parse_expr()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(n),
                    _ => bail!("missing closing )"),
                }
            }
            Some(Tok::Phrase(s)) => Ok(Node::Term {
                field: None,
                value: s,
                quoted: true,
            }),
            Some(Tok::Word(w)) => {
                // `field:"quoted value"` arrives as Word("field:") + Phrase("…")
                // because the lexer breaks the word at the `"`. Recombine.
                if w.ends_with(':') && matches!(self.peek(), Some(Tok::Phrase(_))) {
                    let field = w[..w.len() - 1].to_lowercase();
                    if !field.is_empty() {
                        if let Some(Tok::Phrase(s)) = self.bump() {
                            return Ok(Node::Term {
                                field: Some(field),
                                value: s,
                                quoted: true,
                            });
                        }
                    }
                }
                Ok(parse_word_term(&w)?)
            }
            Some(tok) => bail!("unexpected token: {:?}", tok),
            None => bail!("unexpected end of input"),
        }
    }
}

fn parse_word_term(w: &str) -> Result<Node> {
    if let Some(colon) = w.find(':') {
        // Field names are case-insensitive — less surprising than mixing case-sensitivity rules per field.
        let field = w[..colon].to_lowercase();
        let value = &w[colon + 1..];
        if !field.is_empty() && !value.is_empty() {
            // Numeric comparisons: field:>N field:>=N field:<N field:<=N
            if let Some(rest) = value.strip_prefix(">=") {
                return Ok(Node::Range {
                    field,
                    lo: Bound::Included(rest.into()),
                    hi: Bound::Unbounded,
                });
            }
            if let Some(rest) = value.strip_prefix("<=") {
                return Ok(Node::Range {
                    field,
                    lo: Bound::Unbounded,
                    hi: Bound::Included(rest.into()),
                });
            }
            if let Some(rest) = value.strip_prefix('>') {
                return Ok(Node::Range {
                    field,
                    lo: Bound::Excluded(rest.into()),
                    hi: Bound::Unbounded,
                });
            }
            if let Some(rest) = value.strip_prefix('<') {
                return Ok(Node::Range {
                    field,
                    lo: Bound::Unbounded,
                    hi: Bound::Excluded(rest.into()),
                });
            }
            return Ok(Node::Term {
                field: Some(field),
                value: value.into(),
                quoted: false,
            });
        }
    }
    Ok(Node::Term {
        field: None,
        value: w.into(),
        quoted: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Node {
        parse(s).unwrap().unwrap()
    }

    #[test]
    fn empty_and_star_are_match_all() {
        assert!(parse("").unwrap().is_none());
        assert!(parse("   ").unwrap().is_none());
        assert!(parse("*").unwrap().is_none());
    }

    #[test]
    fn bare_term() {
        match p("error") {
            Node::Term {
                field,
                value,
                quoted,
            } => {
                assert_eq!(field, None);
                assert_eq!(value, "error");
                assert!(!quoted);
            }
            other => panic!("expected Term, got {other:?}"),
        }
    }

    #[test]
    fn field_value_term_lowercases_field() {
        match p("STATUS:500") {
            Node::Term { field, value, .. } => {
                assert_eq!(field.as_deref(), Some("status"));
                assert_eq!(value, "500"); // value case preserved
            }
            other => panic!("expected Term, got {other:?}"),
        }
    }

    #[test]
    fn quoted_field_value_recombines() {
        match p(r#"msg:"a b""#) {
            Node::Term {
                field,
                value,
                quoted,
            } => {
                assert_eq!(field.as_deref(), Some("msg"));
                assert_eq!(value, "a b");
                assert!(quoted);
            }
            other => panic!("expected Term, got {other:?}"),
        }
    }

    #[test]
    fn numeric_range_comparators() {
        let cases = [
            (">100", Bound::Excluded("100".to_string()), Bound::Unbounded),
            (
                ">=100",
                Bound::Included("100".to_string()),
                Bound::Unbounded,
            ),
            ("<100", Bound::Unbounded, Bound::Excluded("100".to_string())),
            (
                "<=100",
                Bound::Unbounded,
                Bound::Included("100".to_string()),
            ),
        ];
        for (suffix, want_lo, want_hi) in cases {
            match p(&format!("latency:{suffix}")) {
                Node::Range { field, lo, hi } => {
                    assert_eq!(field, "latency");
                    assert_eq!(lo, want_lo, "lo for {suffix}");
                    assert_eq!(hi, want_hi, "hi for {suffix}");
                }
                other => panic!("expected Range for {suffix}, got {other:?}"),
            }
        }
    }

    #[test]
    fn implicit_and_binds_tighter_than_or() {
        // `a OR b c` == `a OR (b AND c)`
        match p("a OR b c") {
            Node::Or(alts) => {
                assert_eq!(alts.len(), 2);
                assert!(matches!(alts[1], Node::And(_)));
            }
            other => panic!("expected Or, got {other:?}"),
        }
    }

    #[test]
    fn not_wraps_atom() {
        assert!(matches!(p("-error"), Node::Not(_)));
        assert!(matches!(p("NOT error"), Node::Not(_)));
    }

    #[test]
    fn parens_group() {
        // `(a OR b) c` == AND[ Or[a,b], c ]
        match p("(a OR b) c") {
            Node::And(conj) => {
                assert_eq!(conj.len(), 2);
                assert!(matches!(conj[0], Node::Or(_)));
            }
            other => panic!("expected And, got {other:?}"),
        }
    }

    #[test]
    fn parse_errors() {
        assert!(parse("(a").is_err()); // missing close paren
        assert!(parse("a)").is_err()); // trailing token
        assert!(parse(r#""unterminated"#).is_err()); // lexer error propagates
    }
}
