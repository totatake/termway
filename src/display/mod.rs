pub mod damage;
pub mod framebuffer;
pub mod xvfb;

pub use damage::{DamageRect, DamageTracker};
pub use framebuffer::Framebuffer;
pub use xvfb::XvfbServer;
