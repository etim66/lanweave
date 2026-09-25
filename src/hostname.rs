//! Local computer name for discovery display and `hello` text.
//!
//! The operating system is the only reliable source: shells commonly keep
//! `HOSTNAME` unexported, so environment variables are a fallback only. The
//! raw name is untrusted display text and is cleaned before it is used.

/// Maximum length of a DNS-SD instance label in bytes.
const MAX_INSTANCE_BYTES: usize = 63;
/// Maximum length of a local display name in bytes.
const MAX_DISPLAY_BYTES: usize = 128;

/// Returns the local computer name, free of control characters and bounded.
///
/// Falls back to `COMPUTERNAME`/`HOSTNAME` and finally to `lanweave`, so the
/// result is never empty.
pub(crate) fn local_hostname() -> String {
    let name = clean(&gethostname::gethostname().to_string_lossy());
    if !name.is_empty() {
        return name;
    }

    let fallback = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let fallback = clean(&fallback);
    if fallback.is_empty() {
        "lanweave".to_owned()
    } else {
        fallback
    }
}

/// Returns a DNS-SD-safe instance label that preserves the given casing.
///
/// Characters outside `[A-Za-z0-9_-]` fold into a single dash so an instance
/// name can never contain a label separator; the result is bounded to 63
/// ASCII bytes.
pub(crate) fn instance_label(name: &str) -> String {
    let mut label = String::with_capacity(name.len().min(MAX_INSTANCE_BYTES));
    let mut previous_was_dash = false;

    for character in name.chars() {
        if label.len() >= MAX_INSTANCE_BYTES {
            break;
        }
        if character.is_ascii_alphanumeric() || character == '_' {
            label.push(character);
            previous_was_dash = false;
        } else if !previous_was_dash && !label.is_empty() {
            label.push('-');
            previous_was_dash = true;
        }
    }
    while label.ends_with('-') {
        label.pop();
    }
    label
}

/// Removes control characters and bounds the text to the display limit.
fn clean(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    limit(&filtered, MAX_DISPLAY_BYTES)
}

/// Truncates `text` to at most `max` bytes without splitting a character.
fn limit(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{instance_label, limit, local_hostname};

    #[test]
    fn instance_labels_preserve_case_and_fold_separators() {
        assert_eq!(
            instance_label("etim-HP-EliteBook-845-G7-Notebook-PC"),
            "etim-HP-EliteBook-845-G7-Notebook-PC"
        );
        assert_eq!(instance_label("host.name"), "host-name");
        assert_eq!(instance_label(" Workstation # 1 "), "Workstation-1");
        assert_eq!(instance_label("---"), "");
        assert!(instance_label(&"x".repeat(200)).len() <= 63);
        assert!(instance_label("ééé").is_ascii());
    }

    #[test]
    fn limits_do_not_split_characters() {
        assert_eq!(limit("abc", 5), "abc");
        assert_eq!(limit("café", 4), "caf");
        assert_eq!(limit("café", 5), "café");
    }

    #[test]
    fn local_hostname_is_never_empty_or_control_laden() {
        let name = local_hostname();

        assert!(!name.is_empty());
        assert!(!name.chars().any(char::is_control));
        assert!(name.len() <= 128);
    }
}
