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
}
