// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Log-centric tokenizer: keeps `_`/`-` (and digit-sandwiched `.`) inside major
//! tokens, then emits each sub-segment at the next position so `latency_ms` is
//! findable as `latency`/`ms`. Lowercased; consecutive positions keep phrases consistent.

/// A token plus its 0-based stream position. Positions are consecutive (minors
/// follow their major), which the block engine's phrase verification relies on.
#[derive(Clone, Debug, Default)]
pub struct LogToken {
    pub text: String,
    // Carried for parity with the `tokenize_into` callback; readers use `text`.
    #[allow(dead_code)]
    pub position: u32,
}

#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

#[inline]
fn is_minor_breaker(c: char) -> bool {
    matches!(c, '_' | '-' | '.')
}

/// Tokenize `text` into major + minor segments (see the module docs). Tokens
/// keep their original case; callers lowercase as needed.
pub fn tokenize(text: &str) -> Vec<LogToken> {
    let mut out = Vec::new();
    let mut scratch = Vec::new();
    tokenize_into(text, &mut scratch, |tok, position| {
        out.push(LogToken {
            text: tok.to_string(),
            position,
        });
    });
    out
}

/// Allocation-free core: emits each `(token, position)` as a borrowed slice of
/// `text`. `scratch` is a caller-owned reuse buffer, so the hot loop never allocates.
pub fn tokenize_into<'a>(
    text: &'a str,
    scratch: &mut Vec<(usize, char)>,
    mut emit: impl FnMut(&'a str, u32),
) {
    scratch.clear();
    scratch.extend(text.char_indices());
    let chars = &scratch[..];
    let n = chars.len();
    let mut i = 0;
    let mut next_position = 0u32;

    while i < n {
        // Skip non-word chars.
        while i < n && !is_word_char(chars[i].1) {
            i += 1;
        }
        if i >= n {
            break;
        }

        // ---- pass 1: span the major token (a byte range into `text`) ----
        let start = chars[i].0;
        let mut end = start;
        let mut last_was_digit = false;
        while i < n {
            let c = chars[i].1;
            let keep = if is_word_char(c) {
                last_was_digit = c.is_ascii_digit();
                true
            } else if c == '.' && last_was_digit && i + 1 < n && chars[i + 1].1.is_ascii_digit() {
                last_was_digit = false;
                true
            } else {
                false
            };
            if !keep {
                break;
            }
            end = chars[i].0 + c.len_utf8();
            i += 1;
        }
        let major = &text[start..end];

        emit(major, next_position);
        next_position += 1;

        // ---- pass 2: emit minor sub-segments if major contains breakers ----
        if major.bytes().any(|b| matches!(b, b'_' | b'-' | b'.')) {
            for minor in major.split(is_minor_breaker) {
                if minor.is_empty() || minor.len() == major.len() {
                    continue;
                }
                emit(minor, next_position);
                next_position += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: &str) -> Vec<String> {
        tokenize(input).into_iter().map(|t| t.text).collect()
    }

    // --- core splits ---

    #[test]
    fn whitespace_and_equals() {
        // No minor breakers in any token, so output equals major segments.
        assert_eq!(
            tokens("request completed method=GET status=200"),
            vec!["request", "completed", "method", "GET", "status", "200"]
        );
    }

    #[test]
    fn underscores_emit_major_plus_minors() {
        assert_eq!(
            tokens("latency_ms=210"),
            vec!["latency_ms", "latency", "ms", "210"]
        );
    }

    #[test]
    fn hyphens_emit_major_plus_minors() {
        assert_eq!(
            tokens("service=payment-gateway"),
            vec!["service", "payment-gateway", "payment", "gateway"]
        );
    }

    #[test]
    fn slashes_and_paths() {
        // Slashes are major breakers; no minors needed.
        assert_eq!(
            tokens("path=/api/v1/users"),
            vec!["path", "api", "v1", "users"]
        );
    }

    #[test]
    fn hostnames_split_on_dots() {
        // web-01 has a hyphen, so we also get its minors.
        assert_eq!(
            tokens("host=web-01.prod.example.com"),
            vec!["host", "web-01", "web", "01", "prod", "example", "com"]
        );
    }

    // --- digit-sandwiched dot rule (still works, and now also yields minors) ---

    #[test]
    fn ipv4_address_emits_major_plus_octets() {
        assert_eq!(
            tokens("peer=10.0.244.230"),
            vec!["peer", "10.0.244.230", "10", "0", "244", "230"]
        );
    }

    #[test]
    fn version_string_emits_major_plus_parts() {
        assert_eq!(tokens("v=1.2.3"), vec!["v", "1.2.3", "1", "2", "3"]);
    }

    #[test]
    fn semver_with_prerelease_partially_kept() {
        // Major: 1.0.0-rc (hyphen keeps it together) then 1 alone.
        // Minors of 1.0.0-rc: split on '.' and '-' → 1, 0, 0, rc.
        assert_eq!(
            tokens("version=1.0.0-rc.1"),
            vec!["version", "1.0.0-rc", "1", "0", "0", "rc", "1"]
        );
    }

    #[test]
    fn decimal_latency() {
        assert_eq!(
            tokens("latency=12.5ms"),
            vec!["latency", "12.5ms", "12", "5ms"]
        );
    }

    #[test]
    fn trailing_period_not_kept() {
        assert_eq!(tokens("request completed."), vec!["request", "completed"]);
    }

    #[test]
    fn abbreviations_split() {
        assert_eq!(tokens("Mr. Smith"), vec!["Mr", "Smith"]);
    }

    // --- structured fragments ---

    #[test]
    fn url_with_query_string() {
        assert_eq!(
            tokens("url=https://api.example.com/v1/users?id=42&user=admin"),
            vec![
                "url", "https", "api", "example", "com", "v1", "users", "id", "42", "user", "admin"
            ]
        );
    }

    #[test]
    fn json_fragment() {
        // user_id and abc-123 both emit major + minors.
        assert_eq!(
            tokens(r#"{"user_id":"abc-123","count":5}"#),
            vec!["user_id", "user", "id", "abc-123", "abc", "123", "count", "5"]
        );
    }

    #[test]
    fn brackets() {
        assert_eq!(
            tokens("[ERROR] payment-gateway: down"),
            vec!["ERROR", "payment-gateway", "payment", "gateway", "down"]
        );
    }

    #[test]
    fn quoted_substring() {
        assert_eq!(
            tokens(r#"message="upstream call failed""#),
            vec!["message", "upstream", "call", "failed"]
        );
    }

    // --- identifiers ---

    #[test]
    fn uuid_emits_major_plus_hex_parts() {
        assert_eq!(
            tokens("trace_id=550e8400-e29b-41d4-a716-446655440000"),
            vec![
                "trace_id",
                "trace",
                "id",
                "550e8400-e29b-41d4-a716-446655440000",
                "550e8400",
                "e29b",
                "41d4",
                "a716",
                "446655440000"
            ]
        );
    }

    #[test]
    fn hex_trace_id() {
        // span_id is a minor breaker pair; the hex id has no breakers.
        assert_eq!(
            tokens("span_id=1c56b153d2ca4a5c"),
            vec!["span_id", "span", "id", "1c56b153d2ca4a5c"]
        );
    }

    #[test]
    fn k8s_pod_name() {
        assert_eq!(
            tokens("pod=nginx-deployment-abc12-xyz3p"),
            vec![
                "pod",
                "nginx-deployment-abc12-xyz3p",
                "nginx",
                "deployment",
                "abc12",
                "xyz3p"
            ]
        );
    }

    #[test]
    fn email_splits_on_at_and_dots() {
        assert_eq!(
            tokens("from=user@example.com"),
            vec!["from", "user", "example", "com"]
        );
    }

    // --- stack traces ---

    #[test]
    fn java_stack_trace_frame() {
        assert_eq!(
            tokens("at com.foo.Bar.baz(Bar.java:42)"),
            vec!["at", "com", "foo", "Bar", "baz", "Bar", "java", "42"]
        );
    }

    #[test]
    fn npe_keeps_camelcase_token() {
        assert_eq!(tokens("NullPointerException"), vec!["NullPointerException"]);
    }

    // --- misc ---

    #[test]
    fn case_is_not_altered() {
        assert_eq!(tokens("INFO debug Warn"), vec!["INFO", "debug", "Warn"]);
    }

    #[test]
    fn number_with_unit_no_dot() {
        assert_eq!(
            tokens("size=200ms count=5k"),
            vec!["size", "200ms", "count", "5k"]
        );
    }

    #[test]
    fn memory_addresses() {
        assert_eq!(
            tokens("addr=0x7fff5fbff8a8"),
            vec!["addr", "0x7fff5fbff8a8"]
        );
    }

    #[test]
    fn empty_input() {
        assert!(tokens("").is_empty());
    }

    #[test]
    fn only_separators() {
        assert!(tokens("  /// === ").is_empty());
    }

    // --- the bare `ms` / `latency` use cases the user asked for ---

    #[test]
    fn bare_ms_finds_latency_ms_token() {
        // The whole point of the minor-segment pass: searchers can type `ms`
        // and the inverted index will have it.
        let all = tokens("request latency_ms=210");
        assert!(all.iter().any(|t| t == "ms"), "tokens were: {:?}", all);
        assert!(all.iter().any(|t| t == "latency"), "tokens were: {:?}", all);
        assert!(
            all.iter().any(|t| t == "latency_ms"),
            "tokens were: {:?}",
            all
        );
    }

    #[test]
    fn bare_payment_finds_payment_gateway() {
        let all = tokens("service=payment-gateway");
        assert!(all.iter().any(|t| t == "payment"));
        assert!(all.iter().any(|t| t == "gateway"));
        assert!(all.iter().any(|t| t == "payment-gateway"));
    }
}
