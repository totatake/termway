use std::env;
use termway::compositor::{EngineConfig, TermwayCompositor};
use termway::Result;

fn print_help() {
    println!(
        r#"termway: The Ultimate Terminal GUI Bridge

USAGE:
    termway [OPTIONS] [--] [COMMAND [ARGS...]]

OPTIONS:
    --wayland                 Use native Smithay Wayland Compositor (default)
    --width <PIXELS>          Virtual display width (default: auto from terminal)
    --height <PIXELS>         Virtual display height (default: auto from terminal)
    --cell-size <WxH>         Font cell pixel size (e.g. 10x20, default: auto/env/10x20)
    --fps <FPS>               Target frame rate (default: 30)
    --scale <FACTOR>          Wayland output scale factor (e.g. 1.0, 1.25, 1.5, 2.0; default: 1.0)
    --sixel                   Force Sixel graphics protocol
    --kitty                   Force Kitty graphics protocol
    --jpeg                    Enable Kitty Graphics Protocol JPEG extension (f=101)
    --no-jpeg                 Disable Kitty Graphics Protocol JPEG extension (use standard zlib)
    --jpeg-quality <1-100>    Set JPEG compression quality (default: 80)
    --hud                     Enable real-time performance HUD indicator
    --profile                 Enable performance profiling and latency breakdown
    --no-splash               Disable startup splash screen
    --demo                    Run built-in demo GUI application
    -V, --version             Print version information
    -h, --help                Print this help information

EXAMPLES:
    termway --demo
    termway --scale 1.5 -- gvim
    termway --kitty -- gvim
    termway --cell-size 10x22 -- gvim
    termway -- xclock
"#
    );
}

fn main() -> Result<()> {
    TermwayCompositor::ensure_xdg_runtime_dir();
    termway::terminal::winsize::setup_sigwinch();
    let args: Vec<String> = env::args().collect();
    let mut config = EngineConfig::default();

    let mut explicit_sixel = false;
    let mut explicit_kitty = false;

    let mut i = 1;
    let mut run_demo = false;

    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            print_help();
            return Ok(());
        } else if arg == "-V" || arg == "--version" {
            println!("termway {}", env!("CARGO_PKG_VERSION"));
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
        } else if arg == "--scale" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --scale requires a numeric float argument (e.g. 1.25, 1.5, 2.0)");
                std::process::exit(1);
            }
            match args[i].parse::<f64>() {
                Ok(s) if s >= 0.25 && s <= 4.0 => config.scale = s,
                _ => {
                    eprintln!("Error: --scale must be a float between 0.25 and 4.0 (e.g. 1.25, 1.5, 2.0)");
                    std::process::exit(1);
                }
            }
        } else if arg == "--wayland" {
            // Native Smithay Wayland Compositor is the default and sole engine
        } else if arg == "--sixel" {
            explicit_sixel = true;
        } else if arg == "--kitty" {
            explicit_kitty = true;
        } else if arg == "--jpeg" {
            config.use_jpeg = Some(true);
        } else if arg == "--no-jpeg" {
            config.use_jpeg = Some(false);
        } else if arg == "--jpeg-quality" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --jpeg-quality requires a numeric argument between 1 and 100");
                std::process::exit(1);
            }
            match args[i].parse::<u8>() {
                Ok(q) if (1..=100).contains(&q) => config.jpeg_quality = Some(q),
                _ => {
                    eprintln!("Error: --jpeg-quality must be an integer between 1 and 100");
                    std::process::exit(1);
                }
            }
        } else if arg == "--hud" {
            config.hud = true;
        } else if arg == "--profile" {
            config.profile = true;
        } else if arg == "--no-splash" {
            config.no_splash = true;
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
        let is_atty = unsafe { libc::isatty(libc::STDIN_FILENO) == 1 };
        match query_graphics_protocol() {
            GraphicsProtocol::Kitty => {
                config.use_sixel = false;
            }
            GraphicsProtocol::Sixel => {
                config.use_sixel = true;
            }
            GraphicsProtocol::None => {
                if is_atty {
                    eprintln!(
                        r#"Error: No supported graphics protocol detected in current terminal.
termway requires inline graphics support (Kitty Graphics Protocol or Sixel).

Supported terminals:
  - Ghostty (macOS / Linux)
  - WezTerm (Windows / macOS / Linux)
  - Kitty (macOS / Linux)
  - Windows Terminal (Windows)
  - foot (Linux Wayland)
  - iTerm2 (macOS)
  - xterm (with Sixel support: -ti vt340)

Note: Standard GNOME Terminal, standard Alacritty, and standard Apple Terminal do not support Sixel/Kitty graphics.
If your terminal supports Sixel or Kitty but auto-detection failed, you can explicitly force the protocol:
  termway --sixel [COMMAND...]
  termway --kitty [COMMAND...]"#
                    );
                    std::process::exit(1);
                } else {
                    // Non-TTY (e.g. piped tests/automated capture), default to Sixel
                    config.use_sixel = true;
                }
            }
        }
    }

    if run_demo || config.command.is_empty() {
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
entry.set_position(-1)
Gtk.main()
"#
            .to_string(),
        ];
    }

    if !run_demo && !config.command.is_empty() {
        let cmd_name = &config.command[0];
        if termway::session::find_executable(cmd_name).is_none() {
            eprintln!("Error: Command '{}' not found in PATH.", cmd_name);
            std::process::exit(127);
        }
    }

    let mut compositor = TermwayCompositor::new(config)?;
    if let Err(e) = compositor.run() {
        eprintln!("Error: {}", e);
        drop(compositor);
        std::process::exit(1);
    }

    Ok(())
}

