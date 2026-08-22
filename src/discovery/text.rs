/// Maximum length of an escaped display string in bytes.
const MAX_DISPLAY_BYTES: usize = 128;

/// Escapes controls and bidi characters for safe display, bounded by bytes.
///
/// Long output is cut with a trailing ellipsis at a UTF-8 boundary.
pub(super) fn escape_display(input: &str) -> String {
    let mut output = String::new();

    for character in input.chars() {
        let escaped = if character.is_control() || is_bidi_control(character) {
            format!("\\u{{{:04X}}}", u32::from(character))
        } else {
            character.to_string()
        };

        if output.len() + escaped.len() > MAX_DISPLAY_BYTES {
            if output.len() + 3 <= MAX_DISPLAY_BYTES {
                output.push_str("...");
            }
            break;
        }
        output.push_str(&escaped);
    }

    output
}

/// Truncates `input` to at most `max_bytes` without splitting a character.
pub(super) fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }

    let mut end = max_bytes;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    input[..end].to_owned()
}

/// Returns whether `character` is a Unicode bidirectional control.
fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_DISPLAY_BYTES, escape_display, truncate_utf8};

    #[test]
    fn terminal_and_bidirectional_controls_are_visible_text() {
        let escaped = escape_display("peer\n\x1b[31m\u{202e}txt");

        assert_eq!(escaped, "peer\\u{000A}\\u{001B}[31m\\u{202E}txt");
        assert!(!escaped.chars().any(char::is_control));
    }

    #[test]
    fn escaped_output_is_bounded_at_utf8_boundaries() {
        let escaped = escape_display(&format!("{}\x1b", "é".repeat(MAX_DISPLAY_BYTES)));

        assert!(escaped.len() <= MAX_DISPLAY_BYTES);
        assert!(escaped.is_char_boundary(escaped.len()));
    }

    #[test]
    fn truncation_does_not_split_unicode() {
        assert_eq!(truncate_utf8("abécd", 3), "ab");
        assert_eq!(truncate_utf8("abécd", 4), "abé");
    }
}
