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
}
