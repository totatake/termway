#!/usr/bin/env python3
"""
termway GUI test driver

Provides automated GUI testing for termway:
1. Input injection (keyboard sequences and SGR mouse events over pseudo-terminal)
2. Frame capture (capturing rendered GUI frames as PNG)
3. State inspection (querying window/popup hierarchy as JSON via SIGUSR2)
"""

import os
import sys
import time
import json
import signal
import struct
import fcntl
import termios
import pty
import threading
import argparse

class TermwayTestDriver:
    def __init__(self, binary="./target/release/termway", client_cmd="wireshark", dump_dir="/tmp/termway_test_dumps", cols=120, rows=40, width=1080, height=720):
        self.binary = binary
        self.client_cmd = client_cmd if isinstance(client_cmd, list) else [client_cmd]
        self.dump_dir = dump_dir
        self.cols = cols
        self.rows = rows
        self.width = width
        self.height = height
        self.pid = None
        self.master_fd = None
        self.reader_thread = None
        self.running = False
        os.makedirs(self.dump_dir, exist_ok=True)

    def start(self, env_extra=None):
        """Starts termway inside a virtual pseudo-terminal (PTY)."""
        pid, master_fd = pty.fork()
        if pid == 0:
            # Child process: inside slave PTY
            env = os.environ.copy()
            env["TERM"] = "xterm-256color"
            env["TERMWAY_TEST_DUMP_DIR"] = self.dump_dir
            env["TERMWAY_TEST_STATE_FILE"] = os.path.join(self.dump_dir, "gui_state.json")
            if env_extra:
                env.update(env_extra)

            # Set PTY window size (cols, rows, width_px, height_px)
            ws = struct.pack("HHHH", self.rows, self.cols, self.width, self.height)
            fcntl.ioctl(0, termios.TIOCSWINSZ, ws)

            args = [self.binary] + self.client_cmd
            os.execve(self.binary, args, env)
        else:
            self.pid = pid
            self.master_fd = master_fd
            self.running = True

            def drain():
                while self.running:
                    try:
                        data = os.read(self.master_fd, 65536)
                        if not data:
                            break
                    except OSError:
                        break

            self.reader_thread = threading.Thread(target=drain, daemon=True)
            self.reader_thread.start()

    def send_raw(self, data: bytes):
        """Writes raw bytes directly to the PTY master."""
        if self.master_fd is not None:
            os.write(self.master_fd, data)

    def send_key(self, key: str):
        """Sends a key stroke or sequence."""
        if key == "Enter":
            self.send_raw(b"\r")
        elif key == "Escape":
            self.send_raw(b"\x1b")
        elif key == "Tab":
            self.send_raw(b"\t")
        elif key == "Up":
            self.send_raw(b"\x1b[A")
        elif key == "Down":
            self.send_raw(b"\x1b[B")
        elif key == "Right":
            self.send_raw(b"\x1b[C")
        elif key == "Left":
            self.send_raw(b"\x1b[D")
        else:
            self.send_raw(key.encode("utf-8"))

    def send_mouse(self, col: int, row: int, action="click", button=0):
        """
        Injects standard SGR 1006 mouse event into the terminal.
        - col, row: 1-based terminal cell coordinates
        - action: 'press', 'release', 'click', 'move'
        """
        if action == "press":
            self.send_raw(f"\x1b[<{button};{col};{row}M".encode("ascii"))
        elif action == "release":
            self.send_raw(f"\x1b[<{button};{col};{row}m".encode("ascii"))
        elif action == "click":
            self.send_raw(f"\x1b[<{button};{col};{row}M".encode("ascii"))
            time.sleep(0.04)
            self.send_raw(f"\x1b[<{button};{col};{row}m".encode("ascii"))
        elif action == "move":
            # Motion event with no buttons pressed is button code 35
            self.send_raw(f"\x1b[<35;{col};{row}M".encode("ascii"))

    def capture_screenshot(self) -> str:
        """Triggers screenshot capture via SIGUSR1 and returns path to screenshot."""
        if self.pid is None:
            return ""
        os.kill(self.pid, signal.SIGUSR1)
        time.sleep(0.2)
        latest = os.path.join(self.dump_dir, "termway_capture_latest.png")
        return latest if os.path.exists(latest) else ""

    def query_gui_state(self) -> dict:
        """Triggers GUI state dump via SIGUSR2 and returns parsed JSON dict."""
        if self.pid is None:
            return {}
        state_file = os.path.join(self.dump_dir, "gui_state.json")
        if os.path.exists(state_file):
            try:
                os.remove(state_file)
            except OSError:
                pass

        os.kill(self.pid, signal.SIGUSR2)
        # Poll for file creation
        for _ in range(20):
            time.sleep(0.05)
            if os.path.exists(state_file):
                try:
                    with open(state_file, "r", encoding="utf-8") as f:
                        return json.load(f)
                except (json.JSONDecodeError, OSError):
                    pass
        return {}

    def stop(self):
        """Terminates termway process."""
        self.running = False
        if self.pid is not None:
            try:
                os.kill(self.pid, signal.SIGTERM)
                time.sleep(0.3)
                os.kill(self.pid, signal.SIGKILL)
            except OSError:
                pass
            self.pid = None
        if self.master_fd is not None:
            try:
                os.close(self.master_fd)
            except OSError:
                pass
            self.master_fd = None

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.stop()

def main():
    parser = argparse.ArgumentParser(description="termway GUI Test Automation Runner")
    parser.add_argument("--bin", default="./target/release/termway", help="Path to termway executable")
    parser.add_argument("--client", default="wireshark", help="Client application to spawn")
    parser.add_argument("--dump-dir", default="/tmp/termway_test_dumps", help="Directory for screenshots and state")
    args = parser.parse_args()

    driver = TermwayTestDriver(binary=args.bin, client_cmd=args.client, dump_dir=args.dump_dir)
    print(f"Starting termway with client {args.client}...")
    driver.start()
    try:
        time.sleep(4.0)
        print("Initial GUI state:")
        state = driver.query_gui_state()
        print(json.dumps(state, indent=2))

        print("Capturing initial screenshot...")
        shot = driver.capture_screenshot()
        print(f"Captured: {shot}")

        print("Simulating mouse click on menubar...")
        driver.send_mouse(col=3, row=3, action="click")
        time.sleep(0.5)

        state = driver.query_gui_state()
        print(f"Popups open: {len(state.get('popups', []))}")
        print(json.dumps(state, indent=2))

        shot = driver.capture_screenshot()
        print(f"Captured after click: {shot}")
    finally:
        driver.stop()
        print("Done.")

if __name__ == "__main__":
    main()

