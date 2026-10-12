#!/usr/bin/env python3
"""
Automated Demo Recorder & GIF Generator for Termway.

Scenario recorded:
  1. From Windows Terminal, SSH login to Linux host (optional via --ssh-target)
  2. Launch gnome-terminal via termway (`termway -- gnome-terminal`), kept maximized as background
  3. Launch `xeyes &` in the background from gnome-terminal
  4. Unmaximize xeyes, resize its window into a compact square via its bottom-right handle,
     and drag it to the top-right corner
  5. Move the mouse cursor in smooth circles over top-right xeyes so its eyes spin
  6. Keep xeyes open at top-right and launch `wireshark &` from gnome-terminal
  7. Unmaximize wireshark via its [□] titlebar button (showing gnome-terminal, xeyes, and wireshark)
  8. Minimize wireshark via its [-] button into the bottom taskbar, then restore it from the taskbar
  9. Close wireshark via [X], close top-right xeyes via [X], and exit gnome-terminal (`exit`)
"""

from __future__ import annotations

import argparse
import datetime
import json
import math
import pathlib
import sys
import threading
import time
import traceback
import warnings
from dataclasses import dataclass
from typing import List, Optional, Tuple

warnings.filterwarnings("ignore", category=DeprecationWarning)

import mss
import pyautogui
from PIL import Image, ImageDraw

# Disable PyAutoGUI's default 0.1s pause after every call for smooth mouse circles;
# we manage explicit pauses between scenario actions.
pyautogui.PAUSE = 0.01
pyautogui.FAILSAFE = True

# Termway Window Manager geometry constants (must match src/compositor/window.rs)
TITLEBAR_HEIGHT = 22
BORDER_WIDTH = 1
BUTTON_SIZE = 16

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
CALIBRATION_FILE = SCRIPT_DIR / ".demo_calibration.json"
DEBUG_LOG_FILE = SCRIPT_DIR / "record_demo_debug.log"


def log(msg: str) -> None:
    """Print to console and append to scripts/record_demo_debug.log for post-mortem debugging."""
    ts = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
    line = f"[{ts}] {msg}"
    print(line, flush=True)
    try:
        with DEBUG_LOG_FILE.open("a", encoding="utf-8") as f:
            f.write(line + "\n")
    except Exception:
        pass


def flush_console_input() -> None:
    """
    Drain any keystrokes that may have been queued into the runner console's stdin buffer.
    Prevents PowerShell from reading a queued `exit` command and closing the window when Python exits.
    """
    if sys.platform == "win32":
        try:
            import msvcrt

            while msvcrt.kbhit():
                msvcrt.getwch()
        except Exception:
            pass
    else:
        try:
            import termios

            termios.tcflush(sys.stdin, termios.TCIFLUSH)
        except Exception:
            pass


@dataclass
class TerminalBounds:
    """Screen coordinates (in physical pixels) of the terminal viewport."""

    # Full capture region (includes the terminal canvas + bottom status bar row)
    capture_left: int
    capture_top: int
    capture_width: int
    capture_height: int

    # Termway graphical canvas region (excludes the 1-row bottom taskbar/status bar)
    canvas_left: int
    canvas_top: int
    canvas_width: int
    canvas_height: int

    # Approximate character cell dimensions in pixels (used for taskbar chip clicking)
    cell_width: float
    cell_height: float

    def canvas_pt(self, x: float, y: float) -> Tuple[int, int]:
        """Convert Termway canvas-local coordinates (0..W, 0..H) to screen coordinates."""
        sx = int(round(self.canvas_left + max(0.0, min(float(self.canvas_width - 1), x))))
        sy = int(round(self.canvas_top + max(0.0, min(float(self.canvas_height - 1), y))))
        return sx, sy

    def canvas_rel(self, rx: float, ry: float) -> Tuple[int, int]:
        """Convert normalized canvas coordinates (0.0..1.0) to screen coordinates."""
        return self.canvas_pt(rx * self.canvas_width, ry * self.canvas_height)

    def ssd_maximized_buttons(self) -> Tuple[Tuple[int, int], Tuple[int, int], Tuple[int, int]]:
        """Return (minimize_xy, maximize_xy, close_xy) for a maximized SSD window."""
        w = self.canvas_width
        y = TITLEBAR_HEIGHT // 2  # 11px from top of canvas
        close_pt = self.canvas_pt(w - 12, y)
        max_pt = self.canvas_pt(w - 32, y)
        min_pt = self.canvas_pt(w - 52, y)
        return min_pt, max_pt, close_pt

    def ssd_80pct_geometry(self, offset_x: int = 0, offset_y: int = 0) -> Tuple[int, int, int, int]:
        """
        Return (frame_x, frame_y, frame_w, frame_h) in canvas pixels for an SSD window
        unmaximized to Termway's default 80% centered geometry (plus any drag offset).
        Matches `ManagedWindow::default_80pct_geometry(w, h, true)` in src/compositor/window.rs.
        """
        sw = self.canvas_width
        sh = self.canvas_height
        content_w = max(160, (sw * 8) // 10)
        content_h = max(100, (sh * 8) // 10)
        total_w = content_w + BORDER_WIDTH * 2
        total_h = content_h + TITLEBAR_HEIGHT + BORDER_WIDTH * 2
        frame_x = max(0, (sw - total_w) // 2) + offset_x
        frame_y = max(0, (sh - total_h) // 2) + offset_y
        return frame_x, frame_y, total_w, total_h

    def ssd_80pct_buttons(
        self, offset_x: int = 0, offset_y: int = 0
    ) -> Tuple[Tuple[int, int], Tuple[int, int], Tuple[int, int], Tuple[int, int]]:
        """
        Return (titlebar_center_xy, minimize_xy, maximize_xy, close_xy) for an unmaximized
        80%-centered SSD window with optional drag offset.
        """
        fx, fy, fw, _fh = self.ssd_80pct_geometry(offset_x, offset_y)
        tb_y = fy + TITLEBAR_HEIGHT // 2
        right_x = fx + fw
        title_pt = self.canvas_pt(fx + fw // 2, tb_y)
        close_pt = self.canvas_pt(right_x - 12, tb_y)
        max_pt = self.canvas_pt(right_x - 32, tb_y)
        min_pt = self.canvas_pt(right_x - 52, tb_y)
        return title_pt, min_pt, max_pt, close_pt

    def taskbar_chip_pt(self, chip_index: int) -> Tuple[int, int]:
        """
        Return screen coordinates for clicking a chip on Termway's bottom taskbar row.
        chip_index=1 -> `[1: user@vm-]` (cols ~2..14, center col ~8)
        chip_index=2 -> `[2: xeyes]`    (cols ~16..25, center col ~20)
        chip_index=3 -> `[3: The Wire _]` (cols ~27..41, center col ~34)
        """
        if chip_index == 1:
            col = 8
        elif chip_index == 2:
            col = 20
        else:
            col = 34 + (chip_index - 3) * 15
        x = int(round(self.canvas_left + col * self.cell_width))
        y = int(round(self.canvas_top + self.canvas_height + self.cell_height * 0.5))
        return x, y


class ScreenRecorder:
    """Background screen recorder using `mss` + `Pillow`."""

    def __init__(self, bounds: TerminalBounds, fps: int = 15, draw_cursor: bool = True) -> None:
        self.bounds = bounds
        self.fps = max(1, fps)
        self.draw_cursor = draw_cursor
        self.frames: List[Image.Image] = []
        self.timestamps: List[float] = []
        self._stop_event = threading.Event()
        self._thread: Optional[threading.Thread] = None

    def start(self) -> None:
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._record_loop, daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop_event.set()
        if self._thread is not None:
            self._thread.join(timeout=5.0)

    def _record_loop(self) -> None:
        interval = 1.0 / self.fps
        monitor = {
            "left": self.bounds.capture_left,
            "top": self.bounds.capture_top,
            "width": self.bounds.capture_width,
            "height": self.bounds.capture_height,
        }
        with mss.mss() as sct:
            while not self._stop_event.is_set():
                t0 = time.perf_counter()
                shot = sct.grab(monitor)
                img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")

                if self.draw_cursor:
                    mx, my = pyautogui.position()
                    cx = mx - self.bounds.capture_left
                    cy = my - self.bounds.capture_top
                    if 0 <= cx < self.bounds.capture_width and 0 <= cy < self.bounds.capture_height:
                        draw = ImageDraw.Draw(img)
                        r = 6
                        draw.ellipse(
                            (cx - r, cy - r, cx + r, cy + r),
                            outline=(255, 220, 60),
                            width=2,
                        )
                        draw.ellipse(
                            (cx - 2, cy - 2, cx + 2, cy + 2),
                            fill=(255, 80, 80),
                        )

                self.frames.append(img)
                self.timestamps.append(t0)

                elapsed = time.perf_counter() - t0
                sleep_for = interval - elapsed
                if sleep_for > 0:
                    time.sleep(sleep_for)

    def save_gif(self, output_path: pathlib.Path, max_width: int = 960) -> None:
        if not self.frames:
            raise RuntimeError("No frames were captured.")

        output_path.parent.mkdir(parents=True, exist_ok=True)
        print(f"[recorder] Processing {len(self.frames)} frames into {output_path}...")

        resized: List[Image.Image] = []
        for frame in self.frames:
            if max_width > 0 and frame.width > max_width:
                new_h = int(round(frame.height * (max_width / frame.width)))
                frame = frame.resize((max_width, new_h), Image.Resampling.LANCZOS)
            resized.append(frame)

        # Build a single shared 256-color master palette from evenly spaced keyframes
        # so palette indices remain stable across frames and GIF delta compression cuts size by ~60%.
        sample_count = min(16, len(resized))
        step = max(1, len(resized) // sample_count)
        sample_frames = [resized[i] for i in range(0, len(resized), step)][:sample_count]
        fw, fh = resized[0].size
        sample_sheet = Image.new("RGB", (fw, fh * len(sample_frames)))
        for idx, sf in enumerate(sample_frames):
            sample_sheet.paste(sf, (0, idx * fh))
        master_palette = sample_sheet.quantize(
            colors=256,
            method=Image.Quantize.MEDIANCUT,
            dither=Image.Dither.NONE,
        )

        # Quantize all frames against the shared master palette without dithering so text stays razor-sharp
        processed: List[Image.Image] = [
            frame.quantize(palette=master_palette, dither=Image.Dither.NONE) for frame in resized
        ]

        durations_ms: List[int] = []
        for i in range(len(self.timestamps)):
            if i + 1 < len(self.timestamps):
                dt_ms = int(round((self.timestamps[i + 1] - self.timestamps[i]) * 1000.0))
            else:
                dt_ms = int(round(1000.0 / self.fps))
            durations_ms.append(max(20, min(1000, dt_ms)))

        # Hold the final frame for 1.2s before looping
        durations_ms[-1] = 1200

        processed[0].save(
            output_path,
            save_all=True,
            append_images=processed[1:],
            optimize=True,
            duration=durations_ms,
            loop=0,
        )
        size_mb = output_path.stat().st_size / (1024 * 1024)
        print(f"[recorder] Saved GIF: {output_path} ({size_mb:.2f} MB, {len(processed)} frames)")


def calibrate_terminal_bounds(cols: int = 120, rows: int = 32) -> TerminalBounds:
    """
    Interactive 2-point calibration so clicks land with single-pixel precision
    regardless of Windows Terminal tab bar height, font size, or DPI scaling.
    """
    print("\n=== Terminal Viewport Calibration ===")
    print("1) Make sure Windows Terminal is visible on screen.")
    input(
        "2) Move your mouse cursor to the TOP-LEFT corner of the terminal text area\n"
        "   (just below the Windows Terminal tab bar) and press [Enter] here..."
    )
    x1, y1 = pyautogui.position()
    print(f"   -> Recorded top-left: ({x1}, {y1})")

    input(
        "3) Move your mouse cursor to the BOTTOM-RIGHT corner of the terminal text area\n"
        "   (bottom-right of the status bar / last character row) and press [Enter] here..."
    )
    x2, y2 = pyautogui.position()
    print(f"   -> Recorded bottom-right: ({x2}, {y2})")

    total_w = max(320, x2 - x1)
    total_h = max(240, y2 - y1)
    cell_w = total_w / max(1, cols)
    cell_h = total_h / max(1, rows)

    # Termway leaves 1 bottom row for the taskbar/status bar; Sixel rounds height to a multiple of 6
    avail_rows = max(1, rows - 1)
    canvas_h = int(avail_rows * cell_h)

    bounds = TerminalBounds(
        capture_left=x1,
        capture_top=y1,
        capture_width=total_w,
        capture_height=total_h,
        canvas_left=x1,
        canvas_top=y1,
        canvas_width=total_w,
        canvas_height=canvas_h,
        cell_width=cell_w,
        cell_height=cell_h,
    )

    CALIBRATION_FILE.write_text(json.dumps(bounds.__dict__, indent=2), encoding="utf-8")
    print("Saved calibration to scripts/.demo_calibration.json\n")
    return bounds


def detect_or_load_bounds(recalibrate: bool, cols: int, rows: int) -> TerminalBounds:
    if not recalibrate and CALIBRATION_FILE.is_file():
        data = json.loads(CALIBRATION_FILE.read_text(encoding="utf-8"))
        print("[setup] Loaded saved terminal bounds from scripts/.demo_calibration.json")
        return TerminalBounds(**data)

    if not recalibrate and sys.platform == "win32":
        try:
            import pygetwindow as gw

            wins = [w for w in gw.getWindowsWithTitle("Windows Terminal") if w.width > 200]
            if wins:
                win = wins[0]
                # Windows 11 invisible resize border (~8px left/right/bottom) and WT tab bar (~48px top)
                pad_x = 10
                tab_bar_h = 48
                pad_bottom = 10
                x1 = win.left + pad_x
                y1 = win.top + tab_bar_h
                total_w = max(320, win.width - pad_x * 2)
                total_h = max(240, win.height - tab_bar_h - pad_bottom)
                cell_w = total_w / max(1, cols)
                cell_h = total_h / max(1, rows)
                canvas_h = int((rows - 1) * cell_h)
                print(f"[setup] Auto-detected Windows Terminal window at ({x1}, {y1}) {total_w}x{total_h}")
                return TerminalBounds(
                    capture_left=x1,
                    capture_top=y1,
                    capture_width=total_w,
                    capture_height=total_h,
                    canvas_left=x1,
                    canvas_top=y1,
                    canvas_width=total_w,
                    canvas_height=canvas_h,
                    cell_width=cell_w,
                    cell_height=cell_h,
                )
        except Exception as exc:
            print(f"[setup] Auto-detection fallback to calibration ({exc})")

    return calibrate_terminal_bounds(cols=cols, rows=rows)


if sys.platform == "win32":
    import ctypes
    from ctypes import wintypes

    _INPUT_MOUSE = 0
    _INPUT_KEYBOARD = 1
    _MOUSEEVENTF_MOVE = 0x0001
    _MOUSEEVENTF_LEFTDOWN = 0x0002
    _MOUSEEVENTF_LEFTUP = 0x0004
    _KEYEVENTF_KEYUP = 0x0002
    _KEYEVENTF_UNICODE = 0x0004
    _ULONG_PTR = ctypes.c_ulonglong if ctypes.sizeof(ctypes.c_void_p) == 8 else ctypes.c_ulong

    class _KEYBDINPUT(ctypes.Structure):
        _fields_ = [
            ("wVk", wintypes.WORD),
            ("wScan", wintypes.WORD),
            ("dwFlags", wintypes.DWORD),
            ("time", wintypes.DWORD),
            ("dwExtraInfo", _ULONG_PTR),
        ]

    class _MOUSEINPUT(ctypes.Structure):
        _fields_ = [
            ("dx", wintypes.LONG),
            ("dy", wintypes.LONG),
            ("mouseData", wintypes.DWORD),
            ("dwFlags", wintypes.DWORD),
            ("time", wintypes.DWORD),
            ("dwExtraInfo", _ULONG_PTR),
        ]

    class _HARDWAREINPUT(ctypes.Structure):
        _fields_ = [
            ("uMsg", wintypes.DWORD),
            ("wParamL", wintypes.WORD),
            ("wParamH", wintypes.WORD),
        ]

    class _INPUTUnion(ctypes.Union):
        _fields_ = [
            ("mi", _MOUSEINPUT),
            ("ki", _KEYBDINPUT),
            ("hi", _HARDWAREINPUT),
        ]

    class _INPUT(ctypes.Structure):
        _anonymous_ = ("u",)
        _fields_ = [
            ("type", wintypes.DWORD),
            ("u", _INPUTUnion),
        ]

    def _send_unicode_char_win32(ch: str) -> None:
        code = ord(ch)
        inputs = (_INPUT * 2)()
        inputs[0].type = _INPUT_KEYBOARD
        inputs[0].ki = _KEYBDINPUT(wVk=0, wScan=code, dwFlags=_KEYEVENTF_UNICODE, time=0, dwExtraInfo=0)
        inputs[1].type = _INPUT_KEYBOARD
        inputs[1].ki = _KEYBDINPUT(
            wVk=0,
            wScan=code,
            dwFlags=_KEYEVENTF_UNICODE | _KEYEVENTF_KEYUP,
            time=0,
            dwExtraInfo=0,
        )
        ctypes.windll.user32.SendInput(2, ctypes.byref(inputs), ctypes.sizeof(_INPUT))

    def _move_cursor_win32(x: int, y: int) -> None:
        """
        Move the cursor to (x, y) and inject a hardware-stream MOUSEEVENTF_MOVE packet.
        PyAutoGUI's `moveTo()` only calls `SetCursorPos(x, y)`, which updates the cursor
        position on screen but does NOT trigger `PointerMoved` / VT SGR mouse motion sequences
        in Windows Terminal (WinUI 3 / XAML Islands). Placing the cursor 1px away and nudging
        it onto (x, y) via `SendInput(MOUSEEVENTF_MOVE)` forces Windows to deliver a real
        pointer-motion event on any monitor/DPI configuration.
        """
        tx = int(round(x))
        ty = int(round(y))
        ctypes.windll.user32.SetCursorPos(tx - 1, ty)
        inp = _INPUT()
        inp.type = _INPUT_MOUSE
        inp.mi = _MOUSEINPUT(dx=1, dy=0, mouseData=0, dwFlags=_MOUSEEVENTF_MOVE, time=0, dwExtraInfo=0)
        ctypes.windll.user32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(_INPUT))

    def _mouse_button_win32(down: bool) -> None:
        flag = _MOUSEEVENTF_LEFTDOWN if down else _MOUSEEVENTF_LEFTUP
        inp = _INPUT()
        inp.type = _INPUT_MOUSE
        inp.mi = _MOUSEINPUT(dx=0, dy=0, mouseData=0, dwFlags=flag, time=0, dwExtraInfo=0)
        ctypes.windll.user32.SendInput(1, ctypes.byref(inp), ctypes.sizeof(_INPUT))


def move_cursor(x: int, y: int) -> None:
    """Move cursor to (x, y) and ensure the terminal receives a pointer motion event."""
    if sys.platform == "win32":
        _move_cursor_win32(x, y)
    else:
        pyautogui.moveTo(x, y)


def smooth_move_to(x: int, y: int, duration: float = 0.25, steps: int = 12) -> None:
    """Smoothly animate the cursor from its current position to (x, y)."""
    sx, sy = pyautogui.position()
    steps = max(1, steps)
    dt = max(0.01, duration / steps)
    for i in range(1, steps + 1):
        t = i / steps
        nx = int(round(sx + (x - sx) * t))
        ny = int(round(sy + (y - sy) * t))
        move_cursor(nx, ny)
        time.sleep(dt)


def click_at(x: int, y: int) -> None:
    """Move cursor to (x, y) and perform a left click."""
    move_cursor(x, y)
    time.sleep(0.04)
    if sys.platform == "win32":
        _mouse_button_win32(down=True)
        time.sleep(0.04)
        _mouse_button_win32(down=False)
    else:
        pyautogui.click(x, y)


def double_click_at(x: int, y: int, interval: float = 0.10) -> None:
    """Move cursor to (x, y) and perform a double left click."""
    click_at(x, y)
    time.sleep(interval)
    click_at(x, y)


def type_command(cmd: str, char_interval: float = 0.045, post_wait: float = 0.4) -> None:
    """
    Type a shell command with natural human-like cadence and press Enter.
    On Windows, uses Win32 SendInput with KEYEVENTF_UNICODE so symbols like `@` and `&`
    are typed accurately on Japanese (JIS) as well as US keyboard layouts.
    """
    if sys.platform == "win32":
        for ch in cmd:
            _send_unicode_char_win32(ch)
            time.sleep(char_interval)
    else:
        pyautogui.write(cmd, interval=char_interval)
    time.sleep(0.15)
    pyautogui.press("enter")
    time.sleep(post_wait)


def smooth_drag(start_xy: Tuple[int, int], end_xy: Tuple[int, int], duration: float = 0.65, steps: int = 22) -> None:
    """Perform a smooth multi-step left-button mouse drag so Termway's SGR mouse parser tracks it."""
    sx, sy = start_xy
    ex, ey = end_xy
    smooth_move_to(sx, sy, duration=0.18, steps=8)
    time.sleep(0.08)
    if sys.platform == "win32":
        _mouse_button_win32(down=True)
    else:
        pyautogui.mouseDown(button="left")
    time.sleep(0.08)
    step_sleep = max(0.02, duration / max(1, steps))
    for i in range(1, steps + 1):
        t = i / steps
        # Ease-in-out cubic interpolation
        ease = 3 * (t**2) - 2 * (t**3)
        nx = int(round(sx + (ex - sx) * ease))
        ny = int(round(sy + (ey - sy) * ease))
        move_cursor(nx, ny)
        time.sleep(step_sleep)
    time.sleep(0.08)
    if sys.platform == "win32":
        _mouse_button_win32(down=False)
    else:
        pyautogui.mouseUp(button="left")
    time.sleep(0.25)


def move_mouse_in_circles(
    center_xy: Tuple[int, int],
    radius_x: int,
    radius_y: int,
    revolutions: float = 2.0,
    duration: float = 3.2,
) -> None:
    """
    Move the cursor in smooth circles inside xeyes's window so xeyes tracks the motion.
    Paced at ~25 Hz (40ms/step) so each SGR mouse motion event triggers a clean Sixel frame
    over SSH without saturating the terminal input buffer.
    """
    cx, cy = center_xy
    steps = max(40, int(duration * 25))
    dt = duration / steps
    for i in range(steps + 1):
        angle = 2.0 * math.pi * revolutions * (i / steps)
        px = int(round(cx + radius_x * math.cos(angle)))
        py = int(round(cy + radius_y * math.sin(angle)))
        move_cursor(px, py)
        time.sleep(dt)


def run_scenario(args: argparse.Namespace, bounds: TerminalBounds) -> None:
    recorder = ScreenRecorder(bounds, fps=args.fps, draw_cursor=not args.no_cursor)

    log("[scenario] Focusing terminal window in 2 seconds...")
    time.sleep(2.0)

    # Click inside the terminal to ensure the Windows Terminal window is focused
    init_focus_xy = bounds.canvas_rel(0.25, 0.25)
    click_at(*init_focus_xy)
    time.sleep(0.3)

    # IMPORTANT: If the user ran `record_demo_gif.py` inside a Windows Terminal tab,
    # that tab is currently occupied by `python.exe` (not an interactive shell prompt!).
    # Opening a new tab via Ctrl+Shift+T creates a fresh shell tab at the exact same
    # calibrated coordinates while keeping `python.exe` running in the background tab.
    if not args.no_new_tab:
        log("[scenario] Opening a fresh tab in Windows Terminal (Ctrl+Shift+T) for the demo session...")
        pyautogui.hotkey("ctrl", "shift", "t")
        time.sleep(args.new_tab_wait)
        click_at(*init_focus_xy)
        time.sleep(0.3)

    recorder.start()
    log("[scenario] Recording started.")

    try:
        # Step 1: SSH login from Windows Terminal to Linux
        if not args.no_ssh and args.ssh_target:
            ssh_dest = f"{args.ssh_user}@{args.ssh_target}" if args.ssh_user else args.ssh_target
            ssh_cmd = (
                f"ssh -p {args.ssh_port} {ssh_dest}"
                if args.ssh_port and args.ssh_port != 22
                else f"ssh {ssh_dest}"
            )
            log(f"[step 1/11] Logging in via SSH: {ssh_cmd}")
            if args.ssh_password:
                type_command(ssh_cmd, post_wait=args.ssh_password_wait)
                type_command(args.ssh_password, post_wait=args.ssh_wait)
            else:
                type_command(ssh_cmd, post_wait=args.ssh_wait)

        # Step 2: Launch gnome-terminal via termway (kept maximized as background terminal)
        log(f"[step 2/9] Launching gnome-terminal via termway (maximized): {args.termway_cmd}")
        type_command(args.termway_cmd, post_wait=args.gnome_terminal_wait)

        # Ensure gnome-terminal has keyboard focus (left side of screen, away from top-right xeyes)
        gterm_body_xy = bounds.canvas_rel(0.25, 0.35)
        click_at(*gterm_body_xy)
        time.sleep(0.3)

        # Step 3: Launch xeyes in the background from gnome-terminal
        log("[step 3/9] Launching xeyes & in background...")
        type_command("xeyes &", post_wait=1.5)

        # Step 4: Unmaximize xeyes, resize it into a compact square, and move it to the top-right corner
        log("[step 4/9] Unmaximizing xeyes, resizing to square, and moving to top-right...")
        _, xeyes_max_btn, _ = bounds.ssd_maximized_buttons()
        smooth_move_to(*xeyes_max_btn, duration=0.2)
        time.sleep(0.1)
        click_at(*xeyes_max_btn)
        time.sleep(0.6)

        # Resize xeyes from its 80%-centered bottom-right resize handle into a compact square
        fx, fy, fw, fh = bounds.ssd_80pct_geometry()
        content_w = fw - BORDER_WIDTH * 2
        content_h = fh - TITLEBAR_HEIGHT - BORDER_WIDTH * 2
        target_side = max(190, min(250, int(round(bounds.canvas_height * 0.40))))
        dw = target_side - content_w
        dh = target_side - content_h

        resize_start_xy = bounds.canvas_pt(fx + fw - 10, fy + fh - 10)
        resize_end_xy = bounds.canvas_pt((fx + fw - 10) + dw, (fy + fh - 10) + dh)
        smooth_drag(resize_start_xy, resize_end_xy, duration=0.65, steps=20)
        time.sleep(0.3)

        # Drag xeyes by the left side of its titlebar (fx + 45, fy + 11) into the top-right corner.
        # Termway clamps window drag at the top-right boundary (x = canvas_width, y = 0),
        # so xeyes lands flush in the top-right corner.
        drag_start_xy = bounds.canvas_pt(fx + 45, fy + TITLEBAR_HEIGHT // 2)
        drag_end_xy = bounds.canvas_pt(bounds.canvas_width - 20, 6)
        smooth_drag(drag_start_xy, drag_end_xy, duration=0.65, steps=20)
        time.sleep(0.3)

        # Step 5: Swirl the mouse cursor in smooth circles over top-right xeyes so its eyes spin
        log("[step 5/9] Moving mouse in circles to animate top-right xeyes...")
        xeyes_center_xy = bounds.canvas_pt(
            bounds.canvas_width - (target_side * 0.50) - 4,
            TITLEBAR_HEIGHT + (target_side * 0.50),
        )
        move_mouse_in_circles(
            xeyes_center_xy,
            radius_x=int(target_side * 0.36),
            radius_y=int(target_side * 0.36),
            revolutions=2.5,
            duration=3.2,
        )
        time.sleep(0.4)

        # Step 6: Launch wireshark in the background from gnome-terminal while keeping xeyes open.
        # IMPORTANT: Do NOT click `[2: xeyes]` on the taskbar here — `wireshark` creates its
        # `xdg_toplevel` within ~300ms of launch (before finishing plugin loading), so raising
        # `xeyes` during startup would place `xeyes` above `wireshark` in the Z-order and cause
        # Step 7's top-right `[□]` click to hit `xeyes` instead of `wireshark`!
        log("[step 6/9] Launching wireshark & in background...")
        click_at(*gterm_body_xy)
        time.sleep(0.25)
        type_command("wireshark &", post_wait=args.wireshark_wait)

        # Step 7: Unmaximize wireshark (clicks [□] on top-right of maximized SSD titlebar)
        log("[step 7/9] Unmaximizing wireshark...")
        _, ws_max_btn, _ = bounds.ssd_maximized_buttons()
        smooth_move_to(*ws_max_btn, duration=0.25)
        time.sleep(0.2)
        click_at(*ws_max_btn)
        time.sleep(1.0)

        # Step 8: Minimize wireshark to taskbar, raise xeyes above gnome-terminal via `[2: xeyes]`,
        # then restore wireshark by clicking `[3: The Wire _]` so all 3 windows are visible together
        log("[step 8/9] Minimizing wireshark to taskbar...")
        _, ws_min_btn, _, _ = bounds.ssd_80pct_buttons()
        smooth_move_to(*ws_min_btn, duration=0.25)
        time.sleep(0.2)
        click_at(*ws_min_btn)
        time.sleep(0.8)

        log("[step 8/9] Raising xeyes above gnome-terminal from taskbar...")
        xeyes_chip_xy = bounds.taskbar_chip_pt(chip_index=2)
        smooth_move_to(*xeyes_chip_xy, duration=0.25)
        time.sleep(0.15)
        click_at(*xeyes_chip_xy)
        time.sleep(0.8)

        log("[step 8/9] Restoring wireshark from taskbar...")
        ws_chip_xy = bounds.taskbar_chip_pt(chip_index=3)
        smooth_move_to(*ws_chip_xy, duration=0.3)
        time.sleep(0.2)
        click_at(*ws_chip_xy)
        time.sleep(1.1)

        # Step 9: Close wireshark, close top-right xeyes, and exit gnome-terminal
        log("[step 9/9] Closing wireshark, xeyes, and gnome-terminal...")
        _, _, _, ws_close_btn = bounds.ssd_80pct_buttons()
        smooth_move_to(*ws_close_btn, duration=0.25)
        time.sleep(0.2)
        click_at(*ws_close_btn)
        time.sleep(0.9)

        # Because xeyes was clamped flush into the top-right corner, its [X] button is at
        # the top-right of the canvas (same coordinates as a maximized SSD close button).
        _, _, xeyes_top_right_close_btn = bounds.ssd_maximized_buttons()
        smooth_move_to(*xeyes_top_right_close_btn, duration=0.25)
        time.sleep(0.2)
        click_at(*xeyes_top_right_close_btn)
        time.sleep(0.8)

        click_at(*gterm_body_xy)
        time.sleep(0.25)
        type_command("exit", post_wait=2.2)

    finally:
        recorder.stop()
        log("[scenario] Recording stopped.")
        flush_console_input()

    recorder.save_gif(pathlib.Path(args.output), max_width=args.max_width)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Automatically drive and record the Termway README demo GIF."
    )
    parser.add_argument(
        "--ssh-target",
        default="localhost",
        help="SSH host to connect to in step 1 (default: localhost).",
    )
    parser.add_argument(
        "--ssh-port",
        type=int,
        default=10022,
        help="SSH port to connect to in step 1 (default: 10022).",
    )
    parser.add_argument(
        "--ssh-user",
        default="user",
        help="SSH username in step 1 (default: user).",
    )
    parser.add_argument(
        "--ssh-password",
        default="user",
        help="SSH password in step 1 (default: user; pass '' to skip password entry).",
    )
    parser.add_argument(
        "--ssh-password-wait",
        type=float,
        default=2.8,
        help="Seconds to wait for the SSH password prompt before typing password (default: 2.8).",
    )
    parser.add_argument(
        "--no-ssh",
        action="store_true",
        help="Skip the `ssh` login step if the terminal is already logged into the Linux host.",
    )
    parser.add_argument(
        "--no-new-tab",
        action="store_true",
        help="Do not open a new Windows Terminal tab via Ctrl+Shift+T before recording.",
    )
    parser.add_argument(
        "--new-tab-wait",
        type=float,
        default=1.8,
        help="Seconds to wait after opening a new Windows Terminal tab (default: 1.8).",
    )
    parser.add_argument(
        "--no-pause",
        action="store_true",
        help="Do not wait for Enter before exiting the script.",
    )
    parser.add_argument(
        "--termway-cmd",
        default="termway -- gnome-terminal",
        help="Command used to launch gnome-terminal inside termway (default: 'termway -- gnome-terminal').",
    )
    parser.add_argument(
        "--calibrate",
        action="store_true",
        help="Run interactive 2-point mouse calibration for the Windows Terminal viewport before recording.",
    )
    parser.add_argument(
        "--cols",
        type=int,
        default=120,
        help="Terminal column count for taskbar chip coordinate estimation (default: 120).",
    )
    parser.add_argument(
        "--rows",
        type=int,
        default=32,
        help="Terminal row count for canvas/taskbar height split (default: 32).",
    )
    parser.add_argument(
        "--fps",
        type=int,
        default=15,
        help="Recording frame rate in FPS (default: 15).",
    )
    parser.add_argument(
        "--max-width",
        type=int,
        default=960,
        help="Maximum GIF width in pixels (default: 960; 0 to keep native resolution).",
    )
    parser.add_argument(
        "--no-cursor",
        action="store_true",
        help="Do not draw the cursor highlight indicator on recorded frames.",
    )
    parser.add_argument(
        "--ssh-wait",
        type=float,
        default=1.8,
        help="Seconds to wait after completing SSH login (default: 1.8).",
    )
    parser.add_argument(
        "--gnome-terminal-wait",
        type=float,
        default=3.0,
        help="Seconds to wait for termway splash + gnome-terminal startup (default: 3.0).",
    )
    parser.add_argument(
        "--wireshark-wait",
        type=float,
        default=4.5,
        help="Seconds to wait for wireshark startup (default: 4.5).",
    )
    parser.add_argument(
        "-o",
        "--output",
        default="docs/demo.gif",
        help="Output GIF path (default: docs/demo.gif).",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    try:
        DEBUG_LOG_FILE.write_text("", encoding="utf-8")
    except Exception:
        pass
    log("[main] Starting record_demo_gif.py (debug log: scripts/record_demo_debug.log)")
    try:
        bounds = detect_or_load_bounds(recalibrate=args.calibrate, cols=args.cols, rows=args.rows)
        run_scenario(args, bounds)
        log("[main] Scenario completed successfully.")
    except Exception as exc:
        log(f"[main] ERROR: {exc}")
        log(traceback.format_exc())
    finally:
        flush_console_input()
        if not args.no_pause:
            try:
                input(
                    "\n[record_demo_gif] Debug log saved to scripts/record_demo_debug.log\n"
                    "Press [Enter] to return to the PowerShell prompt..."
                )
            except EOFError:
                pass


if __name__ == "__main__":
    main()

