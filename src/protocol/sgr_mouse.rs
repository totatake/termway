/// Mouse button representation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    None,
    Other(u16),
}

/// Mouse action type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Drag,
    Move,
}

/// Parsed SGR mouse event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub button: MouseButton,
    pub action: MouseAction,
    pub col: u16, // 1-indexed column
    pub row: u16, // 1-indexed row
    pub shift: bool,
    pub meta: bool,
    pub ctrl: bool,
}

/// Parser for xterm SGR 1006 mouse tracking sequences: `\x1b[<{btn};{col};{row}{M|m}`
pub struct SgrMouseParser;

impl SgrMouseParser {
    /// Attempt to parse a single SGR mouse event from the beginning of `input`.
    /// Returns `Some((event, bytes_consumed))` on success, or `None` if no valid sequence matched.
    pub fn parse(input: &str) -> Option<(MouseEvent, usize)> {
        let bytes = input.as_bytes();
        if bytes.len() < 8 {
            return None;
        }

        // Must start with `\x1b[<`
        if &bytes[0..3] != b"\x1b[<" {
            return None;
        }

        // Find terminator 'M' or 'm'
        let term_idx = bytes[3..].iter().position(|&b| b == b'M' || b == b'm')? + 3;
        let is_release_char = bytes[term_idx] == b'm';

        let inner_str = std::str::from_utf8(&bytes[3..term_idx]).ok()?;
        let parts: Vec<&str> = inner_str.split(';').collect();
        if parts.len() != 3 {
            return None;
        }

        let raw_btn: u16 = parts[0].parse().ok()?;
        let col: u16 = parts[1].parse().ok()?;
        let row: u16 = parts[2].parse().ok()?;

        let shift = (raw_btn & 4) != 0;
        let meta = (raw_btn & 8) != 0;
        let ctrl = (raw_btn & 16) != 0;

        let is_motion = (raw_btn & 32) != 0;
        let is_wheel = (raw_btn & 64) != 0;

        let base_btn = raw_btn & 3;

        let (button, action) = if is_wheel {
            let btn = match raw_btn & 65 {
                64 => MouseButton::WheelUp,
                65 => MouseButton::WheelDown,
                _ => MouseButton::Other(raw_btn),
            };
            (btn, MouseAction::Press)
        } else if is_release_char {
            let btn = match base_btn {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                2 => MouseButton::Right,
                _ => MouseButton::None,
            };
            (btn, MouseAction::Release)
        } else if is_motion {
            if raw_btn & 3 == 3 {
                // Motion without buttons pressed (mode 1003h)
                (MouseButton::None, MouseAction::Move)
            } else {
                let btn = match base_btn {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    _ => MouseButton::Other(base_btn),
                };
                (btn, MouseAction::Drag)
            }
        } else {
            // Button Press
            let btn = match base_btn {
                0 => MouseButton::Left,
                1 => MouseButton::Middle,
                2 => MouseButton::Right,
                _ => MouseButton::Other(base_btn),
            };
            (btn, MouseAction::Press)
        };

        let event = MouseEvent {
            button,
            action,
            col,
            row,
            shift,
            meta,
            ctrl,
        };

        Some((event, term_idx + 1))
    }
}
