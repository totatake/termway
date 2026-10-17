use termway::protocol::{MouseAction, MouseButton, SgrMouseParser};

#[test]
fn test_sgr_mouse_left_click_press() {
    let input = "\x1b[<0;42;15M";
    let (event, len) = SgrMouseParser::parse(input).expect("Failed to parse");
    assert_eq!(len, input.len());
    assert_eq!(event.button, MouseButton::Left);
    assert_eq!(event.action, MouseAction::Press);
    assert_eq!(event.col, 42);
    assert_eq!(event.row, 15);
    assert!(!event.shift);
    assert!(!event.meta);
    assert!(!event.ctrl);
}

#[test]
fn test_sgr_mouse_left_click_release() {
    let input = "\x1b[<0;42;15m";
    let (event, len) = SgrMouseParser::parse(input).expect("Failed to parse");
    assert_eq!(len, input.len());
    assert_eq!(event.button, MouseButton::Left);
    assert_eq!(event.action, MouseAction::Release);
    assert_eq!(event.col, 42);
    assert_eq!(event.row, 15);
}

#[test]
fn test_sgr_mouse_drag() {
    let input = "\x1b[<32;50;20M";
    let (event, len) = SgrMouseParser::parse(input).expect("Failed to parse");
    assert_eq!(len, input.len());
    assert_eq!(event.button, MouseButton::Left);
    assert_eq!(event.action, MouseAction::Drag);
    assert_eq!(event.col, 50);
    assert_eq!(event.row, 20);
}

#[test]
fn test_sgr_mouse_wheel() {
    let up_input = "\x1b[<64;10;5M";
    let (up_event, _) = SgrMouseParser::parse(up_input).expect("Failed to parse wheel up");
    assert_eq!(up_event.button, MouseButton::WheelUp);
    assert_eq!(up_event.action, MouseAction::Press);

    let down_input = "\x1b[<65;10;5M";
    let (down_event, _) = SgrMouseParser::parse(down_input).expect("Failed to parse wheel down");
    assert_eq!(down_event.button, MouseButton::WheelDown);
    assert_eq!(down_event.action, MouseAction::Press);
}

#[test]
fn test_sgr_mouse_modifiers() {
    // 0 + 4 (Shift) = 4
    let input = "\x1b[<4;10;10M";
    let (event, _) = SgrMouseParser::parse(input).expect("Failed to parse shift-click");
    assert_eq!(event.button, MouseButton::Left);
    assert!(event.shift);
    assert!(!event.ctrl);
}

#[test]
fn test_sgr_mouse_invalid_inputs() {
    assert!(SgrMouseParser::parse("").is_none());
    assert!(SgrMouseParser::parse("random text").is_none());
    assert!(SgrMouseParser::parse("\x1b[<").is_none());
    assert!(SgrMouseParser::parse("\x1b[<0;0M").is_none()); // only 2 numbers
    assert!(SgrMouseParser::parse("\x1b[<a;b;cM").is_none()); // non-numeric
}

#[test]
fn test_sgr_mouse_pixel_coordinates() {
    let input = "\x1b[<0;1920;1080M";
    let (event, len) = SgrMouseParser::parse(input).expect("Failed to parse high pixel coordinates");
    assert_eq!(len, input.len());
    assert_eq!(event.col, 1920);
    assert_eq!(event.row, 1080);
    assert_eq!(event.button, MouseButton::Left);
    assert_eq!(event.action, MouseAction::Press);
}
