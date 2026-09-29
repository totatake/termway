use std::env;
use termway::engine::{EngineConfig, TermwayEngine};

fn print_help() {
    println!(
        r#"termway: The Ultimate X11-to-Terminal GUI Bridge

USAGE:
    termway [OPTIONS] [--] [COMMAND [ARGS...]]

OPTIONS:
    --width <PIXELS>          Virtual display width (default: auto from terminal)
    --height <PIXELS>         Virtual display height (default: auto from terminal)
    --fps <FPS>               Target frame rate (default: 30)
    --demo                    Run built-in Tkinter demo GUI application
    -h, --help                Print this help information

EXAMPLES:
    termway --demo
    termway -- python3 my_gui_app.py
    termway -- xclock
"#
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let mut config = EngineConfig::default();

    let mut i = 1;
    let mut run_demo = false;

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

    if run_demo {
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

    if config.command.is_empty() {
        println!("No command specified. Running built-in demo...");
        config.command = vec![
            "python3".to_string(),
            "-c".to_string(),
            r#"
import tkinter as tk
root = tk.Tk()
root.title("termway Default")
root.geometry("350x200")
lbl = tk.Label(root, text="Welcome to termway!\nPress Ctrl+C to exit.", font=("Helvetica", 13))
lbl.pack(expand=True)
root.mainloop()
"#.to_string(),
        ];
    }

    let engine = TermwayEngine::new(config);
    if let Err(e) = engine.run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
    
    Ok(())
}
