use std::env;
use termway::engine::{EngineConfig, TermwayEngine};

fn print_help() {
    println!(
        r#"termway: The Ultimate Terminal GUI Bridge

USAGE:
    termway [OPTIONS] [--] [COMMAND [ARGS...]]

OPTIONS:
    --x11, --xvfb             Use legacy X11/Xvfb mode instead of native Wayland Compositor
    --wayland                 Use native Smithay Wayland Compositor (default)
    --width <PIXELS>          Virtual display width (default: auto from terminal)
    --height <PIXELS>         Virtual display height (default: auto from terminal)
    --cell-size <WxH>         Font cell pixel size (e.g. 10x20, default: auto/env/10x20)
    --fps <FPS>               Target frame rate (default: 30)
    --wm                      Start a lightweight Window Manager (openbox in X11 mode)
    --sixel                   Force Sixel graphics protocol
    --kitty                   Force Kitty graphics protocol
    --demo                    Run built-in demo GUI application
    -h, --help                Print this help information

EXAMPLES:
    termway --demo
    termway -- gvim
    termway --kitty -- gvim
    termway --cell-size 10x22 -- gvim
    termway --x11 --wm -- xclock
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    termway::compositor::TermwayCompositor::ensure_xdg_runtime_dir();
    termway::terminal::winsize::setup_sigwinch();
    let args: Vec<String> = env::args().collect();
    let mut config = EngineConfig::default();

    let mut explicit_sixel = false;
    let mut explicit_kitty = false;

    let mut i = 1;
    let mut run_demo = false;
    let mut use_wayland = true;

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
        } else if arg == "--cell-size" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --cell-size requires a WxH argument (e.g. 10x20)");
                std::process::exit(1);
            }
            let val = &args[i];
            let parts: Vec<&str> = val.split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                    config.cell_size = Some((w, h));
                } else {
                    eprintln!("Error: --cell-size format must be WxH (e.g. 10x20)");
                    std::process::exit(1);
                }
            } else {
                eprintln!("Error: --cell-size format must be WxH (e.g. 10x20)");
                std::process::exit(1);
            }
        } else if arg == "--fps" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --fps requires a numeric argument");
                std::process::exit(1);
            }
            config.fps = args[i].parse()?;
        } else if arg == "--wayland" {
            use_wayland = true;
        } else if arg == "--x11" || arg == "--xvfb" {
            use_wayland = false;
        } else if arg == "--sixel" {
            explicit_sixel = true;
        } else if arg == "--kitty" {
            explicit_kitty = true;
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

    // グラフィックスプロトコルの決定:
    // 1. --sixel が明示指定された場合 -> Sixel
    // 2. --kitty が明示指定された場合 -> Kitty
    // 3. 指定がない場合 -> query_graphics_protocol() で環境変数および実機プローブ判定
    if explicit_sixel {
        config.use_sixel = true;
    } else if explicit_kitty {
        config.use_sixel = false;
    } else {
        use termway::terminal::query::{query_graphics_protocol, GraphicsProtocol};
        match query_graphics_protocol() {
            GraphicsProtocol::Kitty => {
                config.use_sixel = false;
            }
            GraphicsProtocol::Sixel | GraphicsProtocol::None => {
                config.use_sixel = true;
            }
        }
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
entry.grab_focus()
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
entry.focus_set()

root.mainloop()
"#.to_string(),
            ];
        }
    }

    if !run_demo && !config.command.is_empty() {
        let cmd_name = &config.command[0];
        if termway::session::find_executable(cmd_name).is_none() {
            eprintln!("Error: Command '{}' not found in PATH.", cmd_name);
            std::process::exit(127);
        }
    }

    if !use_wayland && config.enable_wm && termway::session::find_executable("openbox").is_none() {
        eprintln!("Error: Window Manager 'openbox' not found in PATH. Please install openbox (e.g. 'sudo apt install openbox') or run without --wm.");
        std::process::exit(1);
    }

    if use_wayland {
        let mut compositor = termway::compositor::TermwayCompositor::new(config)?;
        if let Err(e) = compositor.run() {
            eprintln!("Error: {}", e);
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
