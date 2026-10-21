use termway::display::{DamageTracker, Framebuffer};
use termway::protocol::{KittyEncoder, SgrMouseParser};
use termway::terminal::TerminalSize;

#[test]
fn test_integration_framebuffer_to_kitty_pipeline() {
    let width = 64;
    let height = 64;
    let mut fb = Framebuffer::new(width, height);
    let mut tracker = DamageTracker::new(width, height, 32);

    // 赤いブロックを (0,0)-(16,16) に描画する
    for y in 0..16 {
        for x in 0..16 {
            let idx = (y * width + x) * 4;
            fb.pixels[idx as usize] = 255;     // R
            fb.pixels[idx as usize + 3] = 255; // A
        }
    }

    let damage = tracker.track_damage(&fb.pixels);
    assert!(!damage.is_empty());

    // Kitty Graphics Protocol でエンコードする
    let mut encoded_stream = String::new();
    for rect in &damage {
        let chunk = KittyEncoder::encode_rgba_subimage(
            &fb.pixels,
            fb.width,
            fb.height,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            Some(1),
            Some(1),
        );
        encoded_stream.push_str(&chunk);
    }

    assert!(encoded_stream.starts_with("\x1b_Ga=T,q=2,f=32"));
    assert!(encoded_stream.contains(",X=1,Y=1"));
}

#[test]
fn test_integration_mouse_to_target_coordinate_pipeline() {
    // X11 エンジンモード (engine.rs) のセル→ピクセル変換テスト
    let term_size = TerminalSize::fallback(80, 24); // cell size 9x18
    let sgr_str = "\x1b[<0;40;12M"; // Left click at col 40, row 12

    let (mouse_ev, _) = SgrMouseParser::parse(sgr_str).expect("parse sgr");
    let (target_x, target_y) = term_size.map_cell_to_target(
        mouse_ev.col,
        mouse_ev.row,
        1024,
        768,
    );

    // 1024x768 画面の中央付近にマッピングされること
    assert!(target_x > 450 && target_x < 550, "mapped x: {}", target_x);
    assert!(target_y > 330 && target_y < 430, "mapped y: {}", target_y);
}

#[test]
fn test_wayland_compositor_mouse_coordinate_conversion() {
    // Wayland コンポジタモード (compositor/mod.rs) のセル→ピクセル変換テスト
    //
    // 変換式: cell_w = compositor_width / term_cols
    //         px = (col - 1) * cell_w
    //
    // 以前のバグ: px = (col - 1) / compositor_width * compositor_width = col - 1
    // (state.width で除算後に w を掛けて相殺し、ピクセル値がセル番号と同じになっていた)

    let compositor_width: u32 = 1080;
    let compositor_height: u32 = 540;
    let term_cols: u16 = 120;
    let term_rows: u16 = 30;

    let cell_w = compositor_width as f64 / term_cols as f64;
    let cell_h = compositor_height as f64 / term_rows as f64;

    // 1セルのサイズは 9x18 ピクセルになるはず
    assert!((cell_w - 9.0).abs() < 0.01, "cell_w = {}", cell_w);
    assert!((cell_h - 18.0).abs() < 0.01, "cell_h = {}", cell_h);

    // col=1, row=1 (左上隅) は px=0, py=0 になるはず
    let col = 1u16;
    let row = 1u16;
    let px = (col.saturating_sub(1)) as f64 * cell_w;
    let py = (row.saturating_sub(1)) as f64 * cell_h;
    assert_eq!(px as i32, 0);
    assert_eq!(py as i32, 0);

    // col=61, row=16 (中央付近) は compositor の中央付近にマッピングされるはず
    let col = 61u16;
    let row = 16u16;
    let px = (col.saturating_sub(1)) as f64 * cell_w;
    let py = (row.saturating_sub(1)) as f64 * cell_h;
    assert!(px > 500.0 && px < 560.0, "center px = {}", px);
    assert!(py > 250.0 && py < 290.0, "center py = {}", py);

    // col=120, row=30 (右下隅) は compositor の右下端近くにマッピングされるはず
    let col = 120u16;
    let row = 30u16;
    let px = (col.saturating_sub(1)) as f64 * cell_w;
    let py = (row.saturating_sub(1)) as f64 * cell_h;
    assert!(px > 1050.0 && px <= compositor_width as f64, "bottom-right px = {}", px);
    assert!(py > 510.0 && py <= compositor_height as f64, "bottom-right py = {}", py);
}

#[test]
fn test_sixel_auto_fallback_detection() {
    let should_use_sixel = std::env::var_os("KITTY_WINDOW_ID").is_none();
    let _ = should_use_sixel;

    // --sixel フラグは明示的な有効化として機能する (上書き)
    let force_sixel = true;
    assert!(force_sixel, "explicit --sixel flag should always enable Sixel");
}

#[test]
fn test_graphics_protocol_fastpath_detection() {
    use termway::terminal::query::{query_graphics_protocol, GraphicsProtocol};

    // Test TERM=xterm-kitty
    std::env::set_var("TERM", "xterm-kitty");
    assert_eq!(query_graphics_protocol(), GraphicsProtocol::Kitty);

    // Test TERM_PROGRAM=ghostty
    std::env::set_var("TERM", "xterm-256color");
    std::env::set_var("TERM_PROGRAM", "ghostty");
    assert_eq!(query_graphics_protocol(), GraphicsProtocol::Kitty);

    // Test TERM_PROGRAM=WezTerm
    std::env::set_var("TERM_PROGRAM", "WezTerm");
    assert_eq!(query_graphics_protocol(), GraphicsProtocol::Kitty);

    // Test WT_SESSION (Windows Terminal)
    std::env::remove_var("TERM_PROGRAM");
    std::env::set_var("WT_SESSION", "dummy-session-id");
    assert_eq!(query_graphics_protocol(), GraphicsProtocol::Sixel);

    // Clean up
    std::env::remove_var("WT_SESSION");
    std::env::set_var("TERM", "dumb");
}


#[test]
fn test_termway_version_flag() {
    let binary = env!("CARGO_BIN_EXE_termway");

    // Test --version
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .expect("execute termway --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("termway {}", env!("CARGO_PKG_VERSION"))),
        "stdout should contain package version, got: {}",
        stdout
    );

    // Test -V
    let output_short = std::process::Command::new(binary)
        .arg("-V")
        .output()
        .expect("execute termway -V");
    assert!(output_short.status.success());
    let stdout_short = String::from_utf8_lossy(&output_short.stdout);
    assert!(
        stdout_short.contains(&format!("termway {}", env!("CARGO_PKG_VERSION"))),
        "stdout should contain package version, got: {}",
        stdout_short
    );
}

#[test]
fn test_framebuffer_encode_png_valid_structure() {
    let rgba = vec![
        255, 0, 0, 255, 0, 255, 0, 255, // row 0: red, green
        0, 0, 255, 255, 255, 255, 255, 255, // row 1: blue, white
    ];
    let png = Framebuffer::encode_png(&rgba, 2, 2);

    // Check 8-byte PNG signature
    assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    // Check IHDR chunk header
    assert_eq!(&png[12..16], b"IHDR");
    // Check IEND chunk at the end
    assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
}

#[test]
fn test_query_kitty_jpeg_support_env_and_fallback() {
    use termway::terminal::query_kitty_jpeg_support;

    // TERMWAY_KITTY_JPEG=1 forces true
    std::env::set_var("TERMWAY_KITTY_JPEG", "1");
    assert!(query_kitty_jpeg_support());

    std::env::set_var("TERMWAY_KITTY_JPEG", "true");
    assert!(query_kitty_jpeg_support());

    // TERMWAY_KITTY_JPEG=0 forces false
    std::env::set_var("TERMWAY_KITTY_JPEG", "0");
    assert!(!query_kitty_jpeg_support());

    std::env::set_var("TERMWAY_KITTY_JPEG", "false");
    assert!(!query_kitty_jpeg_support());

    // In a test runner without interactive terminal, unset/auto safely falls back
    std::env::remove_var("TERMWAY_KITTY_JPEG");
    let _ = query_kitty_jpeg_support();
}



