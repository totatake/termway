use super::env::log_line;
use super::renderer::TermwayRenderer;
use super::state::TermwayState;

/// Saves the current compositor framebuffer as a timestamped and latest PNG file.
pub fn save_screenshot(renderer: &mut TermwayRenderer, state: &TermwayState) {
    let dump_dir = std::env::var("TERMWAY_SCREENSHOT_DIR")
        .or_else(|_| std::env::var("TERMWAY_TEST_DUMP_DIR"))
        .unwrap_or_else(|_| ".".into());
    let _ = std::fs::create_dir_all(&dump_dir);
    let w = renderer.width();
    let h = renderer.height();
    if let Ok(pixels) = renderer.render(state) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let png_bytes = crate::display::Framebuffer::encode_png(pixels, w, h);
        let path = format!("{}/termway_screenshot_{}.png", dump_dir, ts);
        if std::fs::write(&path, &png_bytes).is_ok() {
            eprintln!("[termway] Screenshot saved: {}", path);
            log_line(format!("[termway] Screenshot saved: {}", path));
        }
        let latest_path = format!("{}/termway_screenshot_latest.png", dump_dir);
        let _ = std::fs::write(&latest_path, &png_bytes);
    }
}

