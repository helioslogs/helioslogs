// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Schema-on-read field helpers: only universal-core fields are stored structurally;
//! everything else is shredded by type into dynamic columns. These route query field
//! names to the right storage shape.

/// Vestigial schema handle: the block engine is schema-on-read, so this is an
/// empty marker kept only to preserve a few read-path signatures.
#[derive(Clone, Copy, Default)]
pub struct Fields;

/// Returns the (empty) schema-on-read handle.
pub fn build_schema() -> Fields {
    Fields
}

/// Universal-core field names — query routing uses this to decide which lookups
/// hit a real column vs. the `dynamic` shred (`dynamic.<name>`).
pub fn is_universal_core_field(name: &str) -> bool {
    matches!(
        name,
        "timestamp" | "message" | "raw" | "source" | "source_raw" | "dynamic"
    )
}

/// `source` is the only field with a raw-cased display variant, used to route a
/// terms agg to the untokenized value.
pub fn raw_field_name(name: &str) -> &'static str {
    match name {
        "source" => "source_raw",
        _ => "",
    }
}

/// Free-text fields (tokenized; substring + phrase queries).
pub fn is_text_field(name: &str) -> bool {
    matches!(name, "message" | "raw")
}

/// String fields — exact match (lowercased). With schema-on-read, only `source`.
pub fn is_string_field(name: &str) -> bool {
    matches!(name, "source" | "source_raw")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universal_core_fields() {
        for f in [
            "timestamp",
            "message",
            "raw",
            "source",
            "source_raw",
            "dynamic",
        ] {
            assert!(is_universal_core_field(f), "{f} should be core");
        }
        // Arbitrary dynamic fields are not core, and matching is case-sensitive.
        assert!(!is_universal_core_field("status"));
        assert!(!is_universal_core_field("Timestamp"));
        assert!(!is_universal_core_field(""));
    }

    #[test]
    fn raw_field_name_only_source() {
        assert_eq!(raw_field_name("source"), "source_raw");
        assert_eq!(raw_field_name("message"), "");
        assert_eq!(raw_field_name("status"), "");
    }

    #[test]
    fn text_vs_string_fields() {
        assert!(is_text_field("message"));
        assert!(is_text_field("raw"));
        assert!(!is_text_field("source"));

        assert!(is_string_field("source"));
        assert!(is_string_field("source_raw"));
        assert!(!is_string_field("message"));
    }
}
