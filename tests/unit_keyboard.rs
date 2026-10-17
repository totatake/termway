use termway::protocol::{KeyCode, KeyboardParser};

#[test]
fn test_keyboard_characters() {
    let (ev, len) = KeyboardParser::parse(b"hello").expect("parse char");
    assert_eq!(len, 1);
    assert_eq!(ev.code, KeyCode::Char('h'));

    let (ev, len) = KeyboardParser::parse("あ".as_bytes()).expect("parse utf8");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::Char('あ'));
}

#[test]
fn test_keyboard_arrow_keys() {
    let (ev, len) = KeyboardParser::parse(b"\x1b[A").expect("parse Up");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::Up);

    let (ev, len) = KeyboardParser::parse(b"\x1b[B").expect("parse Down");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::Down);

    let (ev, len) = KeyboardParser::parse(b"\x1b[C").expect("parse Right");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::Right);

    let (ev, len) = KeyboardParser::parse(b"\x1b[D").expect("parse Left");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::Left);
}

#[test]
fn test_keyboard_special_keys() {
    let (ev, len) = KeyboardParser::parse(b"\r").expect("parse Enter");
    assert_eq!(len, 1);
    assert_eq!(ev.code, KeyCode::Enter);

    let (ev, len) = KeyboardParser::parse(b"\x1b").expect("parse Esc");
    assert_eq!(len, 1);
    assert_eq!(ev.code, KeyCode::Escape);

    let (ev, len) = KeyboardParser::parse(&[0x03]).expect("parse Ctrl+C");
    assert_eq!(len, 1);
    assert_eq!(ev.code, KeyCode::Ctrl('c'));

    let (ev, len) = KeyboardParser::parse(&[0x16]).expect("parse Ctrl+V");
    assert_eq!(len, 1);
    assert_eq!(ev.code, KeyCode::Ctrl('v'));

    let (ev, len) = KeyboardParser::parse(b"\x1b[24~").expect("parse F12");
    assert_eq!(len, 5);
    assert_eq!(ev.code, KeyCode::F12);
}

#[test]
fn test_keyboard_modified_arrow_and_selection_keys() {
    // Shift + Arrows (xterm CSI 1;2<A/B/C/D>)
    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2A").expect("parse Shift+Up");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftUp);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2B").expect("parse Shift+Down");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftDown);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2C").expect("parse Shift+Right");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftRight);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2D").expect("parse Shift+Left");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftLeft);

    // Shift + Home / End
    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2H").expect("parse Shift+Home");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftHome);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;2F").expect("parse Shift+End");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftEnd);

    // Shift + Tab (BackTab: \x1b[Z)
    let (ev, len) = KeyboardParser::parse(b"\x1b[Z").expect("parse Shift+Tab");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::ShiftTab);

    // Ctrl + Arrows (xterm CSI 1;5<A/B/C/D>)
    let (ev, len) = KeyboardParser::parse(b"\x1b[1;5C").expect("parse Ctrl+Right");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlRight);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;5D").expect("parse Ctrl+Left");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlLeft);

    // Ctrl + Shift + Arrows (xterm CSI 1;6<A/B/C/D>)
    let (ev, len) = KeyboardParser::parse(b"\x1b[1;6C").expect("parse Ctrl+Shift+Right");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlShiftRight);

    let (ev, len) = KeyboardParser::parse(b"\x1b[1;6D").expect("parse Ctrl+Shift+Left");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlShiftLeft);

    // Shift modified tilde keys
    let (ev, len) = KeyboardParser::parse(b"\x1b[3;2~").expect("parse Shift+Delete");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftDelete);

    let (ev, len) = KeyboardParser::parse(b"\x1b[5;2~").expect("parse Shift+PageUp");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftPageUp);

    let (ev, len) = KeyboardParser::parse(b"\x1b[6;2~").expect("parse Shift+PageDown");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::ShiftPageDown);

    // rxvt shifted arrow keys
    let (ev, len) = KeyboardParser::parse(b"\x1b[a").expect("parse rxvt Shift+Up");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::ShiftUp);

    let (ev, len) = KeyboardParser::parse(b"\x1b[d").expect("parse rxvt Shift+Left");
    assert_eq!(len, 3);
    assert_eq!(ev.code, KeyCode::ShiftLeft);

    // Unrecognized CSI sequence consumed gracefully without residue
    let (ev, len) = KeyboardParser::parse(b"\x1b[?2004h").expect("parse unrecognized CSI");
    assert_eq!(len, 8);
    assert_eq!(ev.code, KeyCode::Ignore);

    // Incomplete escape sequence waits for more bytes
    assert!(KeyboardParser::parse(b"\x1b[1;").is_none());
}

#[test]
fn test_keyboard_japanese_utf8_and_partial_sequences() {
    let text = "日本語テスト";
    let mut buf = text.as_bytes().to_vec();
    // Append an incomplete trailing UTF-8 byte (e.g., first byte of 'あ' = 0xE3)
    buf.push(0xE3);

    let mut parsed_chars = Vec::new();
    while let Some((ev, consumed)) = KeyboardParser::parse(&buf) {
        if let KeyCode::Char(c) = ev.code {
            parsed_chars.push(c);
        }
        buf.drain(..consumed);
    }

    let result: String = parsed_chars.into_iter().collect();
    assert_eq!(result, "日本語テスト");
    // Only the incomplete 1-byte prefix of 'あ' remains waiting for the next read
    assert_eq!(buf, vec![0xE3]);

    // Once the remaining 2 bytes of 'あ' (0x81, 0x82) arrive, it parses cleanly
    buf.extend_from_slice(&[0x81, 0x82]);
    let (ev, consumed) = KeyboardParser::parse(&buf).expect("parse completed 'あ'");
    assert_eq!(consumed, 3);
    assert_eq!(ev.code, KeyCode::Char('あ'));
}

#[test]
fn test_kitty_keyboard_protocol() {
    use termway::compositor::input::map_keycode_to_evdev;

    // Ctrl+Tab (\x1b[9;5u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[9;5u").expect("parse Ctrl+Tab");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlTab);
    assert!(ev.is_press);
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+Tab");
    assert_eq!(sc, 15);
    assert!(!shift);
    assert!(ctrl);

    // Ctrl+Shift+Tab (\x1b[9;6u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[9;6u").expect("parse Ctrl+Shift+Tab");
    assert_eq!(len, 6);
    assert_eq!(ev.code, KeyCode::CtrlShiftTab);
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+Shift+Tab");
    assert_eq!(sc, 15);
    assert!(shift);
    assert!(ctrl);

    // Shift+Enter (\x1b[13;2u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[13;2u").expect("parse Shift+Enter");
    assert_eq!(len, 7);
    assert_eq!(ev.code, KeyCode::ShiftEnter);
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Shift+Enter");
    assert_eq!(sc, 28);
    assert!(shift);
    assert!(!ctrl);

    // Ctrl+Enter (\x1b[13;5u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[13;5u").expect("parse Ctrl+Enter");
    assert_eq!(len, 7);
    assert_eq!(ev.code, KeyCode::CtrlEnter);
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+Enter");
    assert_eq!(sc, 28);
    assert!(!shift);
    assert!(ctrl);

    // Ctrl+Shift+P (\x1b[112;6u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[112;6u").expect("parse Ctrl+Shift+P");
    assert_eq!(len, 8);
    assert_eq!(ev.code, KeyCode::CtrlShift('p'));
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+Shift+P");
    assert_eq!(sc, 25); // KEY_P
    assert!(shift);
    assert!(ctrl);

    // Ctrl+; (\x1b[59;5u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[59;5u").expect("parse Ctrl+;");
    assert_eq!(len, 7);
    assert_eq!(ev.code, KeyCode::Ctrl(';'));
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+;");
    assert_eq!(sc, 39); // KEY_SEMICOLON
    assert!(!shift);
    assert!(ctrl);

    // Ctrl+/ (\x1b[47;5u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[47;5u").expect("parse Ctrl+/");
    assert_eq!(len, 7);
    assert_eq!(ev.code, KeyCode::Ctrl('/'));
    let (sc, shift, ctrl) = map_keycode_to_evdev(ev.code).expect("map Ctrl+/");
    assert_eq!(sc, 53); // KEY_SLASH
    assert!(!shift);
    assert!(ctrl);

    // Release event with event_type = 3 (\x1b[112;6:3u)
    let (ev, len) = KeyboardParser::parse(b"\x1b[112;6:3u").expect("parse release");
    assert_eq!(len, 10);
    assert_eq!(ev.code, KeyCode::CtrlShift('p'));
    assert!(!ev.is_press);
}

