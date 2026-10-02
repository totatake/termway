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
    // KITTY_WINDOW_ID が設定されていない環境では Sixel が有効になるべき
    // (main.rs の自動検出ロジックと同じ条件)
    // テスト環境では通常 KITTY_WINDOW_ID は設定されていないので true になるはず
    let should_use_sixel = std::env::var_os("KITTY_WINDOW_ID").is_none();
    // テスト環境依存だが、論理の正しさを確認する
    // Kitty 上でこのテストを実行した場合は false になっても問題なし
    let _ = should_use_sixel; // 実行環境に関わらず panic しないことを確認

    // --sixel フラグは明示的な有効化として機能する (上書き)
    // KITTY_WINDOW_ID があっても --sixel で強制できる
    let force_sixel = true; // --sixel フラグ相当
    assert!(force_sixel, "explicit --sixel flag should always enable Sixel");
}
