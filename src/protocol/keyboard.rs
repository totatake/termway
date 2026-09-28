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
                if bytes.len() >= 3 {
                    match bytes[2] {
                        b'A' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::Up,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        b'B' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::Down,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        b'C' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::Right,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        b'D' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::Left,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        b'H' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::Home,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        b'F' => {
                            return Some((
                                KeyEvent {
                                    code: KeyCode::End,
                                    is_press: true,
                                },
                                3,
                            ))
                        }
                        _ => {}
                    }

                    // Tilde sequences: `\x1b[3~` (Delete), `\x1b[5~` (PageUp), `\x1b[6~` (PageDown)
                    if bytes.len() >= 4 && bytes[3] == b'~' {
                        let code = match bytes[2] {
                            b'1' => KeyCode::Home,
                            b'3' => KeyCode::Delete,
                            b'4' => KeyCode::End,
                            b'5' => KeyCode::PageUp,
                            b'6' => KeyCode::PageDown,
                            _ => return None,
                        };
                        return Some((
                            KeyEvent {
                                code,
                                is_press: true,
                            },
                            4,
                        ));
                    }
                }
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
