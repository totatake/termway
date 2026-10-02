use std::os::unix::io::OwnedFd;
use smithay::{
    delegate_xwayland_shell,
    utils::{Logical, Rectangle},
    wayland::{
        selection::SelectionTarget,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        xwm::{Reorder, ResizeEdge, X11Wm, XwmHandler, XwmId},
        X11Surface,
    },
};

use super::state::TermwayState;

impl XWaylandShellHandler for TermwayState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

impl XwmHandler for TermwayState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("X11Wm not initialized")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {
    }
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // XWayland が自動的にサイズ決定するので、ここで geometry を上書きしない
        let _ = window.set_mapped(true);
        // フォーカスを取得してマウス・キーボードイベントがウィンドウへ届くようにする
        let _ = window.set_activated(true);
        if !self.x11_windows.iter().any(|w| w == &window) {
            self.x11_windows.push(window);
        }
        self.dirty = true;
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_windows.retain(|w| w != &window);
        self.dirty = true;
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_windows.retain(|w| w != &window);
        self.dirty = true;
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // アプリが要求した x/y/w/h を尊重する。
        // w/h が未指定の場合はターミナル全体サイズを強制せず、現在のジオメトリをそのまま保つ。
        let current = window.geometry();
        let (cur_w, cur_h) = (current.size.w as u32, current.size.h as u32);
        // ジオメトリが 0 の場合（初期化直後）はアプリのデフォルトサイズとして 300 を使う
        let (cur_w, cur_h) = (
            if cur_w == 0 { 300 } else { cur_w },
            if cur_h == 0 { 300 } else { cur_h },
        );

        let rect = Rectangle::new(
            (x.unwrap_or(0), y.unwrap_or(0)).into(),
            (w.unwrap_or(cur_w) as i32, h.unwrap_or(cur_h) as i32).into(),
        );
        let _ = window.configure(rect);
        self.dirty = true;
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {}

    fn resize_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32, _resize_edge: ResizeEdge) {}
    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}
    fn send_selection(&mut self, _xwm: XwmId, _selection: SelectionTarget, _mime_type: String, _fd: OwnedFd) {}
}

delegate_xwayland_shell!(TermwayState);
