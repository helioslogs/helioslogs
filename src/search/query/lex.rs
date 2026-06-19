// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Token types + a hand-rolled lexer (whitespace, parens, quoted strings, boolean
//! keywords). `-` always emits `Tok::Not`; the parser decides prefix vs. identifier.

use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub(super) enum Tok {
    Word(String),
    Phrase(String),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

pub(super) fn lex(s: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(Tok::RParen);
            i += 1;
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    s.push(chars[i + 1]);
                    i += 2;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            if i >= chars.len() {
                bail!("unterminated quote");
            }
            i += 1; // skip closing "
            out.push(Tok::Phrase(s));
            continue;
        }
        if c == '-' {
            out.push(Tok::Not);
            i += 1;
            continue;
        }
        let mut w = String::new();
        while i < chars.len() {
            let ch = chars[i];
            if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' {
                break;
            }
            w.push(ch);
            i += 1;
        }
        if w.is_empty() {
            i += 1;
            continue;
        }
        match w.as_str() {
            "AND" | "and" => out.push(Tok::And),
            "OR" | "or" => out.push(Tok::Or),
            "NOT" | "not" => out.push(Tok::Not),
            _ => out.push(Tok::Word(w)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<String> {
        lex(s)
            .unwrap()
            .iter()
            .map(|t| match t {
                Tok::Word(w) => format!("W:{w}"),
                Tok::Phrase(p) => format!("P:{p}"),
                Tok::And => "AND".into(),
                Tok::Or => "OR".into(),
                Tok::Not => "NOT".into(),
                Tok::LParen => "(".into(),
                Tok::RParen => ")".into(),
            })
            .collect()
    }

    #[test]
    fn bare_words() {
        assert_eq!(kinds("error timeout"), ["W:error", "W:timeout"]);
    }

    #[test]
    fn booleans_case_insensitive() {
        assert_eq!(kinds("a AND b OR c"), ["W:a", "AND", "W:b", "OR", "W:c"]);
        assert_eq!(kinds("a and b or c"), ["W:a", "AND", "W:b", "OR", "W:c"]);
        assert_eq!(kinds("a NOT b"), ["W:a", "NOT", "W:b"]);
    }

    #[test]
    fn dash_is_not_token() {
        assert_eq!(kinds("-error"), ["NOT", "W:error"]);
    }

    #[test]
    fn parens_split_adjacent_words() {
        assert_eq!(kinds("(a b)"), ["(", "W:a", "W:b", ")"]);
        // No whitespace needed around parens.
        assert_eq!(kinds("a(b)"), ["W:a", "(", "W:b", ")"]);
    }

    #[test]
    fn quoted_phrase_with_spaces() {
        assert_eq!(kinds(r#""hello world""#), ["P:hello world"]);
    }

    #[test]
    fn quote_escapes() {
        // \" -> literal quote, \\ -> literal backslash.
        assert_eq!(kinds(r#""a\"b""#), [r#"P:a"b"#]);
        assert_eq!(kinds(r#""a\\b""#), [r"P:a\b"]);
    }

    #[test]
    fn field_value_word_breaks_at_quote() {
        // The lexer breaks `field:"x"` into Word("field:") + Phrase("x").
        assert_eq!(kinds(r#"msg:"a b""#), ["W:msg:", "P:a b"]);
    }

    #[test]
    fn unterminated_quote_errors() {
        assert!(lex(r#""no close"#).is_err());
    }

    #[test]
    fn whitespace_only_is_empty() {
        assert!(lex("   \t  ").unwrap().is_empty());
    }
}
