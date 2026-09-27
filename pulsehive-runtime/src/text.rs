//! Crate-private text helpers shared by the runtime's context assembly.
//!
//! Deliberately not part of the public surface: [`truncate`] is an internal
//! rendering detail of the Record and Perceive phases, shared so experience
//! extraction and perception cut a string the same way.

/// Truncate a string to at most `max_len` bytes, appending "..." if truncated.
///
/// The cut index is moved back to the nearest UTF-8 character boundary so a
/// multibyte character straddling `max_len` never panics and the truncated
/// prefix stays valid UTF-8.
pub(crate) fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── UTF-8-safe truncation tests ──────────────────────────────────

    #[test]
    fn test_truncate_multibyte_cut_floors_to_char_boundary() {
        // "あ" is 3 bytes: an 11-byte cut lands mid-character.
        let s = "あ".repeat(10);
        assert_eq!(truncate(&s, 11), format!("{}...", "あ".repeat(3)));
        // U+1F600 is 4 bytes: a 6-byte cut splits the second emoji.
        assert_eq!(truncate("😀😀😀", 6), "😀...");
    }

    #[test]
    fn test_truncate_exact_boundary_and_short_inputs() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "hel...");
        // A cut exactly on a character boundary keeps that character.
        assert_eq!(truncate("あいう", 6), format!("{}...", "あい"));
    }

    #[test]
    fn test_truncate_never_panics_across_all_cut_points() {
        let s = "a漢b😀cé"; // 1-, 2-, 3- and 4-byte characters
        for cut in 0..=s.len() {
            let out = truncate(s, cut);
            assert!(out.ends_with("...") || out.len() == s.len());
        }
    }

    /// The boundary perception actually cuts at: a 2-byte and a 4-byte
    /// character straddling byte 500 must floor to the preceding boundary, and
    /// one ending exactly on it survives whole.
    #[test]
    fn truncate_cuts_multibyte_at_500_boundary() {
        // `é` (2 bytes) starts at byte 499, so byte 500 falls inside it.
        let two_byte = format!("{}é{}", "a".repeat(499), "z".repeat(64));
        assert_eq!(truncate(&two_byte, 500), format!("{}...", "a".repeat(499)));

        // `😀` (4 bytes) starts at byte 497, so byte 500 falls inside it.
        let four_byte = format!("{}😀{}", "a".repeat(497), "z".repeat(64));
        assert_eq!(truncate(&four_byte, 500), format!("{}...", "a".repeat(497)));

        // The same 4-byte character ending exactly on byte 500 is kept whole.
        let on_boundary = format!("{}😀{}", "a".repeat(496), "z".repeat(64));
        assert_eq!(
            truncate(&on_boundary, 500),
            format!("{}...", "a".repeat(496) + "😀")
        );
    }
}
