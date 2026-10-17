//! Smithay protocol handler implementations for `TermwayState`, grouped by protocol domain.
//!
//! Smithay requires every handler trait to be implemented on the single compositor state
//! type, so the handlers stay on `TermwayState`; this module only separates them into
//! cohesive files. Domain logic (window stacking, focus, minimize) is delegated to
//! `WindowManager` where possible.

mod compositor;
mod seat;
mod xdg_shell;

use smithay::{
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_output,
    delegate_seat, delegate_shm, delegate_viewporter, delegate_xdg_decoration, delegate_xdg_shell,
};

use super::state::TermwayState;

delegate_compositor!(TermwayState);
delegate_xdg_shell!(TermwayState);
delegate_shm!(TermwayState);
delegate_seat!(TermwayState);
delegate_output!(TermwayState);
delegate_data_device!(TermwayState);
delegate_xdg_decoration!(TermwayState);
delegate_fractional_scale!(TermwayState);
delegate_viewporter!(TermwayState);
