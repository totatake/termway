use std::env;
use termway::engine::{EngineConfig, TermwayEngine};

fn print_help() {
    println!(
        r#"termway: The Ultimate X11-to-Terminal GUI Bridge

USAGE:
    termway [OPTIONS] [--] [COMMAND [ARGS...]]

OPTIONS:
    --wayland                 Use native Smithay Wayland Compositor instead of X11/Xvfb
    --width <PIXELS>          Virtual display width (default: auto from terminal)
    --height <PIXELS>         Virtual display height (default: auto from terminal)
    --fps <FPS>               Target frame rate (default: 30)
    --wm                      Start a lightweight Window Manager (openbox)
    --sixel                   Use Sixel graphics instead of Kitty protocol
    --demo                    Run built-in Tkinter demo GUI application
    -h, --help                Print this help information

EXAMPLES:
    termway --demo
    termway --wayland -- weston-terminal
    termway --wm -- xclock
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let mut config = EngineConfig::default();

    // 端末が Kitty かどうかを環境変数で判定する。
    // KITTY_WINDOW_ID が存在しない場合は Sixel をデフォルトで有効にする。
    // --sixel フラグで明示的に上書きすることも引き続きできる。
    if std::env::var_os("KITTY_WINDOW_ID").is_none() {
        config.use_sixel = true;
    }

    let mut i = 1;
    let mut run_demo = false;
    let mut use_wayland = false;

    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            print_help();
            return Ok(());
        } else if arg == "--width" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --width requires a numeric argument");
                std::process::exit(1);
            }
            config.width = Some(args[i].parse()?);
        } else if arg == "--height" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --height requires a numeric argument");
                std::process::exit(1);
            }
            config.height = Some(args[i].parse()?);
        } else if arg == "--fps" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --fps requires a numeric argument");
                std::process::exit(1);
            }
            config.fps = args[i].parse()?;
        } else if arg == "--wayland" {
            use_wayland = true;
        } else if arg == "--sixel" {
            config.use_sixel = true;
        } else if arg == "--wm" {
            config.enable_wm = true;
        } else if arg == "--demo" {
            run_demo = true;
        } else if arg == "--" {
            i += 1;
            config.command = args[i..].to_vec();
            break;
        } else {
            // Unrecognized argument, treat as command
            config.command = args[i..].to_vec();
            break;
        }
        i += 1;
    }

    if run_demo || config.command.is_empty() {
        if use_wayland {
            config.command = vec![
                "python3".to_string(),
                "-c".to_string(),
                r#"
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk

win = Gtk.Window(title="termway Wayland Demo")
win.set_default_size(400, 300)
win.connect("destroy", Gtk.main_quit)

box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=15)
win.add(box)

lbl = Gtk.Label(label="Smithay Native Wayland Compositor Demo")
box.pack_start(lbl, True, True, 10)

count = 0
btn = Gtk.Button(label="Click Me!")
def on_click(b):
    global count
    count += 1
    b.set_label(f"Clicked: {count} times!")
btn.connect("clicked", on_click)
box.pack_start(btn, True, True, 10)

entry = Gtk.Entry()
entry.set_text("Type here...")
box.pack_start(entry, True, True, 10)

win.show_all()
Gtk.main()
"#.to_string(),
            ];
        } else {
            config.command = vec![
                "python3".to_string(),
                "-c".to_string(),
                r#"
import tkinter as tk

root = tk.Tk()
root.title("termway Demo")
root.geometry("400x300")

count = 0

def on_click():
    global count
    count += 1
    label.config(text=f"Clicked: {count} times!", fg="blue")

label = tk.Label(root, text="termway: GUI inside Terminal!", font=("Helvetica", 14))
label.pack(pady=20)

btn = tk.Button(root, text="Click Me!", font=("Helvetica", 12), bg="lightgreen", command=on_click)
btn.pack(pady=10)

entry = tk.Entry(root, font=("Helvetica", 12))
entry.insert(0, "Type here...")
entry.pack(pady=10)

root.mainloop()
"#.to_string(),
            ];
        }
    }

    if use_wayland {
        let mut compositor = termway::compositor::TermwayCompositor::new(config)?;
        if let Err(e) = compositor.run() {
            eprintln!("Compositor Error: {}", e);
            std::process::exit(1);
        }
    } else {
        let mut engine = TermwayEngine::new(config);
        if let Err(e) = engine.run() {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}
