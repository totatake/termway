pub mod kitty;
pub mod jpeg;
pub mod sgr_mouse;
pub mod keyboard;

pub use kitty::{KittyEncoder, KittyStreamEncoder};
pub use jpeg::{JpegEncoder, TINY_1X1_JPEG_BASE64};
pub mod sixel;
pub use sixel::{SixelEncoder, SixelStreamEncoder};
pub use sgr_mouse::{MouseButton, MouseAction, MouseEvent, SgrMouseParser};
pub use keyboard::{KeyCode, KeyEvent, KeyboardParser};
