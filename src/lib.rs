pub mod error;
pub mod protocol;
pub mod terminal;
pub mod display;
pub mod backend;
pub mod compositor;
pub mod session;

pub use error::{Result, TermwayError};

pub mod engine {
    pub use crate::compositor::EngineConfig;
}
