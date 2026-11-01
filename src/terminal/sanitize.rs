//! Terminal string sanitization utilities.

/// Sanitizes a string for safe output to terminal control sequences (OSC titles, status bars, etc.).
/// Strips ASCII C0 control characters (0x00..=0x1F, 0x7F) and C1 control characters (0x80..=0x9F)
/// to prevent terminal escape injection attacks (e.g. premature OSC termination via ESC, BEL, or ST).
pub fn sanitize_terminal_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        let u = c as u32;
        // Skip C0 controls (0x00..=0x1F, 0x7F) and C1 controls (0x80..=0x9F)
        if (u <= 0x1F) || (u == 0x7F) || (0x80..=0x9F).contains(&u) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Returns the estimated terminal display column width for a character.
/// Wide characters (e.g. CJK Ideographs, Hiragana, Katakana, Fullwidth forms) take 2 columns;
/// ASCII and typical Latin characters take 1 column. Zero-width/control characters take 0.
pub fn char_column_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x20 || (0x7F..=0x9F).contains(&u) {
        0
    } else if (0x1100..=0x115F).contains(&u) // Hangul Jamo
        || (0x2E80..=0xA4CF).contains(&u) // CJK Radicals, Kangxi, Ideographs, Hiragana, Katakana, Bopomofo, Hangul Syllables
        || (0xAC00..=0xD7A3).contains(&u) // Hangul Syllables
        || (0xF900..=0xFAFF).contains(&u) // CJK Compatibility Ideographs
        || (0xFE10..=0xFE19).contains(&u) // Vertical forms
        || (0xFE30..=0xFE6F).contains(&u) // CJK Compatibility Forms
        || (0xFF00..=0xFF60).contains(&u) // Fullwidth Forms
        || (0xFFE0..=0xFFE6).contains(&u) // Fullwidth Signs
        || (0x20000..=0x2FFFD).contains(&u) // CJK Extension B/C/D/E/F
        || (0x30000..=0x3FFFD).contains(&u) // CJK Extension G/H/I
    {
        2
    } else {
        1
    }
}

/// Returns the estimated terminal display column width of a string.
pub fn str_column_width(s: &str) -> usize {
    s.chars().map(char_column_width).sum()
}

/// Truncates a string such that its terminal display column width does not exceed `max_width`.
pub fn truncate_to_column_width(s: &str, max_width: usize) -> String {
    let mut cur_w = 0;
    let mut out = String::new();
    for c in s.chars() {
        let w = char_column_width(c);
        if cur_w + w > max_width {
            break;
        }
        cur_w += w;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_terminal_text() {
        // Plain text
        assert_eq!(sanitize_terminal_text("Hello World!"), "Hello World!");

        // Injected ESC, BEL, ST, newlines
        let malicious = "evil\x1b]52;c;AAAA\x07title\nwith\rcontrols\x7Fand\u{0080}c1\u{009F}test";
        assert_eq!(sanitize_terminal_text(malicious), "evil]52;c;AAAAtitlewithcontrolsandc1test");

        // Unicode (Japanese) should be preserved
        assert_eq!(sanitize_terminal_text("日本語タイトル"), "日本語タイトル");
    }

    #[test]
    fn test_char_and_str_column_width() {
        assert_eq!(str_column_width("hello"), 5);
        assert_eq!(str_column_width("日本語"), 6);
        assert_eq!(str_column_width("Hi! 日本語"), 10);

        assert_eq!(truncate_to_column_width("日本語タイトル", 8), "日本語タ");
        assert_eq!(truncate_to_column_width("日本語タイトル", 7), "日本語");
    }
}
