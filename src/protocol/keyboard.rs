/// Parsed keyboard key event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Ctrl(char),

    // Shift modified navigation keys
    ShiftUp,
    ShiftDown,
    ShiftRight,
    ShiftLeft,
    ShiftHome,
    ShiftEnd,
    ShiftPageUp,
    ShiftPageDown,
    ShiftDelete,
    ShiftTab,

    // Ctrl modified navigation keys
    CtrlUp,
    CtrlDown,
    CtrlRight,
    CtrlLeft,
    CtrlHome,
    CtrlEnd,

    // Ctrl + Shift modified navigation keys
    CtrlShiftUp,
    CtrlShiftDown,
    CtrlShiftRight,
    CtrlShiftLeft,
    CtrlShiftHome,
    CtrlShiftEnd,

    // Internal ignored/consumed key (for unrecognized escape sequences)
    Ignore,
}

/// Keyboard event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub is_press: bool,
}

pub struct KeyboardParser;

impl KeyboardParser {
    /// Parse next key event from input byte slice.
    /// Returns `Some((key_event, bytes_consumed))`.
    pub fn parse(bytes: &[u8]) -> Option<(KeyEvent, usize)> {
        if bytes.is_empty() {
            return None;
        }

        // Multi-byte escape sequences
        if bytes[0] == 0x1b {
            if bytes.len() == 1 {
                return Some((
                    KeyEvent {
                        code: KeyCode::Escape,
                        is_press: true,
                    },
                    1,
                ));
            }

            if bytes[1] == b'[' {
                // Look for CSI terminator byte (ASCII 0x40..=0x7E, e.g. '@'..='~')
                let term_pos = bytes[2..].iter().position(|&b| (0x40..=0x7e).contains(&b));
                if let Some(rel_idx) = term_pos {
                    let term_idx = 2 + rel_idx;
                    let term_char = bytes[term_idx];
                    let params = std::str::from_utf8(&bytes[2..term_idx]).unwrap_or("");
                    let total_len = term_idx + 1;

                    let code = match term_char {
                        b'A' => match params {
                            "" => KeyCode::Up,
                            "1;2" => KeyCode::ShiftUp,
                            "1;5" => KeyCode::CtrlUp,
                            "1;6" => KeyCode::CtrlShiftUp,
                            _ => KeyCode::Up,
                        },
                        b'B' => match params {
                            "" => KeyCode::Down,
                            "1;2" => KeyCode::ShiftDown,
                            "1;5" => KeyCode::CtrlDown,
                            "1;6" => KeyCode::CtrlShiftDown,
                            _ => KeyCode::Down,
                        },
                        b'C' => match params {
                            "" => KeyCode::Right,
                            "1;2" => KeyCode::ShiftRight,
                            "1;5" => KeyCode::CtrlRight,
                            "1;6" => KeyCode::CtrlShiftRight,
                            _ => KeyCode::Right,
                        },
                        b'D' => match params {
                            "" => KeyCode::Left,
                            "1;2" => KeyCode::ShiftLeft,
                            "1;5" => KeyCode::CtrlLeft,
                            "1;6" => KeyCode::CtrlShiftLeft,
                            _ => KeyCode::Left,
                        },
                        b'H' => match params {
                            "" => KeyCode::Home,
                            "1;2" => KeyCode::ShiftHome,
                            "1;5" => KeyCode::CtrlHome,
                            "1;6" => KeyCode::CtrlShiftHome,
                            _ => KeyCode::Home,
                        },
                        b'F' => match params {
                            "" => KeyCode::End,
                            "1;2" => KeyCode::ShiftEnd,
                            "1;5" => KeyCode::CtrlEnd,
                            "1;6" => KeyCode::CtrlShiftEnd,
                            _ => KeyCode::End,
                        },
                        b'Z' => KeyCode::ShiftTab,
                        // rxvt shifted cursor keys
                        b'a' => KeyCode::ShiftUp,
                        b'b' => KeyCode::ShiftDown,
                        b'c' => KeyCode::ShiftRight,
                        b'd' => KeyCode::ShiftLeft,
                        b'~' => {
                            let parts: Vec<&str> = params.split(';').collect();
                            match parts.as_slice() {
                                ["1"] => KeyCode::Home,
                                ["3"] => KeyCode::Delete,
                                ["4"] => KeyCode::End,
                                ["5"] => KeyCode::PageUp,
                                ["6"] => KeyCode::PageDown,
                                ["1", "2"] => KeyCode::ShiftHome,
                                ["3", "2"] => KeyCode::ShiftDelete,
                                ["4", "2"] => KeyCode::ShiftEnd,
                                ["5", "2"] => KeyCode::ShiftPageUp,
                                ["6", "2"] => KeyCode::ShiftPageDown,
                                ["1", "5"] => KeyCode::CtrlHome,
                                ["4", "5"] => KeyCode::CtrlEnd,
                                ["5", "5"] => KeyCode::PageUp,
                                ["6", "5"] => KeyCode::PageDown,
                                ["1", "6"] => KeyCode::CtrlShiftHome,
                                ["4", "6"] => KeyCode::CtrlShiftEnd,
                                _ => KeyCode::Ignore,
                            }
                        }
                        _ => KeyCode::Ignore,
                    };

                    return Some((KeyEvent { code, is_press: true }, total_len));
                } else {
                    // Sequence is incomplete if buffer is short
                    if bytes.len() < 16 {
                        return None;
                    }
                }
            }

            // SS3 sequences: \x1bO<key>
            if bytes[1] == b'O' {
                if bytes.len() < 3 {
                    return None;
                }
                let code = match bytes[2] {
                    b'A' => KeyCode::Up,
                    b'B' => KeyCode::Down,
                    b'C' => KeyCode::Right,
                    b'D' => KeyCode::Left,
                    b'H' => KeyCode::Home,
                    b'F' => KeyCode::End,
                    b'a' => KeyCode::CtrlUp,
                    b'b' => KeyCode::CtrlDown,
                    b'c' => KeyCode::CtrlRight,
                    b'd' => KeyCode::CtrlLeft,
                    _ => KeyCode::Ignore,
                };
                return Some((KeyEvent { code, is_press: true }, 3));
            }

            // Standalone escape or unrecognized escape
            return Some((
                KeyEvent {
                    code: KeyCode::Escape,
                    is_press: true,
                },
                1,
            ));
        }

        // Single byte control characters
        match bytes[0] {
            b'\r' | b'\n' => Some((
                KeyEvent {
                    code: KeyCode::Enter,
                    is_press: true,
                },
                1,
            )),
            b'\t' => Some((
                KeyEvent {
                    code: KeyCode::Tab,
                    is_press: true,
                },
                1,
            )),
            0x7f | 0x08 => Some((
                KeyEvent {
                    code: KeyCode::Backspace,
                    is_press: true,
                },
                1,
            )),
            1..=26 => {
                // Ctrl+A (1) through Ctrl+Z (26)
                let c = (bytes[0] + b'a' - 1) as char;
                Some((
                    KeyEvent {
                        code: KeyCode::Ctrl(c),
                        is_press: true,
                    },
                    1,
                ))
            }
            _ => {
                // UTF-8 character
                let s = std::str::from_utf8(bytes).ok()?;
                let ch = s.chars().next()?;
                Some((
                    KeyEvent {
                        code: KeyCode::Char(ch),
                        is_press: true,
                    },
                    ch.len_utf8(),
                ))
            }
        }
    }
}
