use termway::protocol::SixelEncoder;

#[test]
fn test_sixel_blank_image() {
    let rgba = vec![0u8; 4 * 4 * 4]; // 4x4 completely transparent/black image
    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, 4, 4);

    // It should start with DCS 7;0;q"1;1;4;4
    assert!(encoded.starts_with("\x1bP7;0;q\"1;1;4;4"));
    // It should define color 0
    assert!(encoded.contains("#0;2;0;0;0"));
    // It should have some blank RLE `!4?` (4 empty pixels)
    assert!(encoded.contains("!4N"));
    // It should end with ST
    assert!(encoded.ends_with("\x1b\\"));
}

#[test]
fn test_sixel_rle() {
    let mut rgba = vec![0u8; 10 * 4]; // 10x1 image
    for i in 0..10 {
        rgba[i * 4] = 255;   // Red
        rgba[i * 4 + 1] = 0; // Green
        rgba[i * 4 + 2] = 0; // Blue
        rgba[i * 4 + 3] = 255;
    }
    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, 10, 1);
    
    // We expect color 224 (111_000_00 = 224)
    assert!(encoded.contains("#224;2;100;0;0"));
    
    // RLE for 10 pixels of 1 row high -> mask 1 -> character 64 '@'
    // !10@
    assert!(encoded.contains("!10@"));
}

#[test]
fn test_sixel_multiple_bands() {
    let mut rgba = vec![0u8; 4 * 10 * 4]; // 4x10 image (requires 2 bands: 6 + 4)
    for y in 0..10 {
        for x in 0..4 {
            let i = (y * 4 + x) * 4;
            rgba[i] = 0;
            rgba[i+1] = 255; // Green
            rgba[i+2] = 0;
            rgba[i+3] = 255;
        }
    }
    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, 4, 10);
    // Green = 000_111_00 = 28
    assert!(encoded.contains("#28;2;0;100;0"));
    // Band 1: 6 rows of green -> mask 63 -> char 126 '~'
    assert!(encoded.contains("!4~"));
    // Band 2: 4 rows of green -> mask 15 -> char 78 'N'
    assert!(encoded.contains("!4N"));
    // LF separating bands
    assert!(encoded.contains("-"));
}

#[test]
fn test_sixel_performance_1080x720() {
    let w = 1080;
    let h = 720;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = (x % 256) as u8;
            rgba[i + 1] = (y % 256) as u8;
            rgba[i + 2] = ((x + y) % 256) as u8;
            rgba[i + 3] = 255;
        }
    }
    let mut out = Vec::with_capacity((w * h) as usize / 2);
    let mut quantized = Vec::with_capacity((w * h) as usize);

    // Warmup
    SixelEncoder::encode_fullscreen_frame_into(&rgba, w, h, &mut out, &mut quantized);

    let start = std::time::Instant::now();
    for _ in 0..5 {
        SixelEncoder::encode_fullscreen_frame_into(&rgba, w, h, &mut out, &mut quantized);
    }
    let elapsed = start.elapsed() / 5;
    eprintln!("\n[BENCHMARK] 1080x720 Sixel zero-allocation encode avg took: {:?}, size: {} KB", elapsed, out.len() / 1024);
    assert!(out.len() > 1000);
}

#[test]
fn test_sixel_solid_band_fastpath() {
    let w = 100;
    let h = 12; // 2 bands of 6 lines
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // Fill completely with solid blue (0, 0, 255)
    for i in (0..rgba.len()).step_by(4) {
        rgba[i] = 0;
        rgba[i + 1] = 0;
        rgba[i + 2] = 255;
        rgba[i + 3] = 255;
    }
    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, w, h);
    // Blue is color 3 (000_000_11)
    assert!(encoded.contains("#3;2;0;0;100"));
    // Full 6-row run of 100 width should emit !100~
    assert!(encoded.contains("!100~"));
}

