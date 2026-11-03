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
    --jpeg-quality <1-100>    Set JPEG compression quality (default: 75)
    --pixel-mouse             Enable SGR-Pixels (DEC 1016h) 1px precision mouse tracking
    --no-pixel-mouse          Disable SGR-Pixels mouse tracking (use standard cell mode)
    --hud                     Enable real-time performance HUD indicator
    --profile                 Enable performance profiling and latency breakdown
    --no-splash               Disable startup splash screen
    --demo                    Run built-in demo GUI application
    --query                   Print diagnostic terminal capability report and exit
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

fn print_capability_report() -> Result<()> {
    use termway::terminal::query::{query_graphics_protocol, query_kitty_jpeg_support, query_pixel_mouse_support, GraphicsProtocol};
    use termway::terminal::TerminalSize;

    let term_size = TerminalSize::query().unwrap_or_else(|_| TerminalSize::fallback(80, 24));
    let proto = query_graphics_protocol();
    let proto_str = match proto {
        GraphicsProtocol::Kitty => "Kitty Graphics Protocol",
        GraphicsProtocol::Sixel => "Sixel",
        GraphicsProtocol::None => "None (Unsupported)",
    };

    let jpeg_support = if proto == GraphicsProtocol::Kitty {
        if query_kitty_jpeg_support() { "Supported (f=101)" } else { "Unsupported / Disabled" }
    } else {
        "N/A (Sixel)"
    };

    let pixel_mouse = if query_pixel_mouse_support() {
        "Supported (SGR-Pixels DECSET 1016)"
    } else {
        "Fallback (SGR Cell DECSET 1006)"
    };

    let xwayland_info = match termway::compositor::env::find_xwayland_binary() {
        Some(path) => format!("Available ({})", path.display()),
        None => "Not found in PATH (X11 clients disabled)".to_string(),
    };

    let dbus_info = match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(addr) if !addr.is_empty() => format!("Connected ({})", addr),
        _ => "Not set in environment".to_string(),
    };

    let polkit_agent = match termway::session::find_executable("polkit-gnome-authentication-agent-1")
        .or_else(|| termway::session::find_executable("lxpolkit"))
        .or_else(|| termway::session::find_executable("mate-polkit")) {
        Some(p) => format!("Available ({})", p.display()),
        None => "Not found in standard paths".to_string(),
    };

    println!("termway diagnostic capability report:");
    println!("  Terminal Size     : {}x{} cells, font cell {}x{} px, total {}x{} px",
        term_size.cols, term_size.rows, term_size.cell_width_px, term_size.cell_height_px, term_size.width_px, term_size.height_px);
    println!("  Graphics Protocol : {}", proto_str);
    println!("  Kitty JPEG Support: {}", jpeg_support);
    println!("  Mouse Tracking    : {}", pixel_mouse);
    println!("  XWayland Server   : {}", xwayland_info);
    println!("  D-Bus Session     : {}", dbus_info);
    println!("  Polkit Agent      : {}", polkit_agent);

    Ok(())
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
        } else if arg == "--query" {
            print_capability_report()?;
            return Ok(());
        } else if arg == "--width" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --width requires a numeric argument >= 320");
                std::process::exit(1);
            }
            match args[i].parse::<u32>() {
                Ok(w) if w >= 320 => config.width = Some(w),
                _ => {
                    eprintln!("Error: --width must be an integer >= 320");
                    std::process::exit(1);
                }
            }
        } else if arg == "--height" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --height requires a numeric argument >= 240");
                std::process::exit(1);
            }
            match args[i].parse::<u32>() {
                Ok(h) if h >= 240 => config.height = Some(h),
                _ => {
                    eprintln!("Error: --height must be an integer >= 240");
                    std::process::exit(1);
                }
            }
        } else if arg == "--cell-size" {
            i += 1;
            if i >= args.len() {
                eprintln!("Error: --cell-size requires a WxH argument (e.g. 10x20)");
                std::process::exit(1);
            }
            let val = &args[i];
            let parts: Vec<&str> = val.split(['x', 'X']).collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                    if w > 0 && h > 0 {
                        config.cell_size = Some((w, h));
                    } else {
                        eprintln!("Error: --cell-size dimensions must be greater than zero");
                        std::process::exit(1);
                    }
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
                eprintln!("Error: --fps requires a numeric argument between 1 and 240");
                std::process::exit(1);
            }
            match args[i].parse::<u32>() {
                Ok(fps) if (1..=240).contains(&fps) => config.fps = fps,
                _ => {
                    eprintln!("Error: --fps must be an integer between 1 and 240");
                    std::process::exit(1);
                }
            }
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
        } else if arg == "--pixel-mouse" {
            config.pixel_mouse = Some(true);
        } else if arg == "--no-pixel-mouse" {
            config.pixel_mouse = Some(false);
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
        } else if arg.starts_with('-') {
            eprintln!("Error: unrecognized option '{}'. Run 'termway --help' for usage.", arg);
            std::process::exit(1);
        } else {
            // Non-option argument, treat as start of command
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
        if config.use_jpeg == Some(true) || config.jpeg_quality.is_some() {
            eprintln!("Warning: --jpeg / --jpeg-quality is only applicable to Kitty graphics protocol and is ignored with --sixel.");
        }
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
        if termway::session::find_executable("python3").is_none() {
            eprintln!("Error: The built-in demo requires 'python3' with PyGObject (python3-gi) and GTK 3.0.");
            eprintln!("Please install python3-gi (e.g. 'sudo apt install python3-gi gir1.2-gtk-3.0') or specify a command to run (e.g. 'termway -- xclock').");
            std::process::exit(127);
        }
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

    let mut compositor = match TermwayCompositor::new(config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };
    if let Err(e) = compositor.run() {
        eprintln!("Error: {}", e);
        drop(compositor);
        std::process::exit(1);
    }

    Ok(())
}

