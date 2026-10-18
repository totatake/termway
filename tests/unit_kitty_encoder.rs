use termway::protocol::KittyEncoder;

#[test]
fn test_clear_all_images() {
    assert_eq!(KittyEncoder::clear_all_images(), "\x1b_Ga=d,d=a\x1b\\");
}

#[test]
fn test_encode_rgba_small_image() {
    // 2x2 red RGBA image
    let rgba = vec![
        255, 0, 0, 255,   255, 0, 0, 255,
        255, 0, 0, 255,   255, 0, 0, 255,
    ];
    let encoded = KittyEncoder::encode_rgba_image(&rgba, 2, 2, None, None);

    // Verify protocol structure
    assert!(encoded.starts_with("\x1b_Ga=T,q=2,f=32,s=2,v=2"));
    assert!(encoded.ends_with("\x1b\\"));
    assert!(encoded.contains(",m=0;"));
}

#[test]
fn test_encode_rgba_subimage() {
    // 4x4 image
    let mut full = vec![0u8; 4 * 4 * 4];
    // Fill pixel (1, 1) with green
    let idx = (4 + 1) * 4;
    full[idx + 1] = 255;
    full[idx + 3] = 255;

    // Extract 2x2 subimage starting at (1, 1)
    let encoded = KittyEncoder::encode_rgba_subimage(&full, 4, 4, 1, 1, 2, 2, Some(5), Some(10));
    assert!(encoded.starts_with("\x1b_Ga=T,q=2,f=32,s=2,v=2,X=5,Y=10,m=0;"));
}

#[test]
fn test_chunked_transfer_for_large_image() {
    // Large image > 4096 base64 chars
    let large_rgba = vec![128u8; 64 * 64 * 4];
    let encoded = KittyEncoder::encode_rgba_image(&large_rgba, 64, 64, None, None);

    assert!(encoded.contains("m=1;"));
    assert!(encoded.contains("m=0;"));
}

#[test]
fn test_encode_fullscreen_frame() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    let rgba = vec![200u8; 32 * 32 * 4];
    let encoded = KittyEncoder::encode_fullscreen_frame(&rgba, 32, 32, 80, 24, 1);

    // Must prepend cursor home sequence to align to top-left of terminal and specify o=z for compression
    assert!(encoded.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648,s=32,v=32,c=80,r=24"));
    assert!(encoded.ends_with("\x1b\\"));

    // Extract payload between ';' and '\x1b\'
    let semi_idx = encoded.find(';').expect("control sequence terminator ';' not found");
    let end_idx = encoded.rfind("\x1b\\").expect("escape terminator not found");
    let b64_payload = &encoded[semi_idx + 1..end_idx];

    let compressed = BASE64.decode(b64_payload).expect("valid base64");
    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).expect("valid zlib");
    assert_eq!(decompressed, rgba);
}

#[test]
fn test_encode_fullscreen_frame_large_chunked_roundtrip() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    // Create a 256x256 image with varied pixel values so it compresses to >4KB
    let mut rgba = vec![0u8; 256 * 256 * 4];
    for (i, p) in rgba.iter_mut().enumerate() {
        *p = ((i * 37) % 256) as u8;
    }

    let encoded = KittyEncoder::encode_fullscreen_frame(&rgba, 256, 256, 120, 40, 42);
    assert!(encoded.starts_with("\x1b[H\x1b_Ga=T,q=2,i=42,p=1,C=1,f=32,o=z,z=-2147483648,s=256,v=256,c=120,r=40"));
    assert!(encoded.ends_with("\x1b\\"));

    // Verify chunking occurred and recombine base64 payload
    let mut recombined_b64 = String::new();
    let mut pos = 0;
    while let Some(chunk_start) = encoded[pos..].find("\x1b_G") {
        let abs_start = pos + chunk_start;
        let semi_rel = encoded[abs_start..].find(';').unwrap();
        let term_rel = encoded[abs_start..].find("\x1b\\").unwrap();
        let payload = &encoded[abs_start + semi_rel + 1 .. abs_start + term_rel];
        recombined_b64.push_str(payload);
        pos = abs_start + term_rel + 2;
    }
    let compressed = BASE64.decode(&recombined_b64).expect("valid base64");
    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).expect("valid zlib");
    let mut expected_rgba = rgba.clone();
    termway::protocol::KittyStreamEncoder::quantize_rgb565(&rgba, &mut expected_rgba);
    assert_eq!(decompressed, expected_rgba);
}

#[test]
fn test_kitty_performance_1080x720() {
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
    let mut b64 = String::with_capacity(128 * 1024);
    let mut out = String::with_capacity(128 * 1024);

    // Warmup
    KittyEncoder::encode_fullscreen_frame_into(&rgba, w, h, 120, 40, 1, &mut b64, &mut out);

    let start = std::time::Instant::now();
    for _ in 0..5 {
        KittyEncoder::encode_fullscreen_frame_into(&rgba, w, h, 120, 40, 1, &mut b64, &mut out);
    }
    let elapsed = start.elapsed() / 5;
    eprintln!("\n[BENCHMARK] 1080x720 Kitty zlib encode avg took: {:?}, size: {} KB", elapsed, out.len() / 1024);
    assert!(out.len() > 1000);
}

#[test]
fn test_kitty_partial_placement_structure() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 160u32;
    let height = 100u32;
    let mut rgba = vec![10u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Frame 0: First frame without damage must establish full-screen canvas
    encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, None, false, &mut output,
    );
    assert!(output.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648"));

    // Modify a 20x10 area at (20, 30)
    for y in 30..40 {
        for x in 20..40 {
            let idx = ((y * width + x) * 4) as usize;
            rgba[idx] = 200;
            rgba[idx + 1] = 50;
            rgba[idx + 2] = 80;
            rgba[idx + 3] = 255;
        }
    }

    let damage = [DamageRect {
        x: 20,
        y: 30,
        width: 20,
        height: 10,
    }];

    // Frame 1: First appearance of damage registers stationary candidate and safely emits full frame
    let is_partial_1 = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        16,
        10,
        10,
        10,
        Some(&damage),
        false,
        &mut output,
    );
    assert!(!is_partial_1, "First frame of new bbox should emit full frame");
    assert!(output.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1"));

    // Frame 2: Consecutive identical damage (e.g. stationary video player) emits partial placement
    let is_partial_2 = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        16,
        10,
        10,
        10,
        Some(&damage),
        false,
        &mut output,
    );
    assert!(is_partial_2, "Stationary bbox must emit partial placement");

    // Cursor position for (x=20, y=30) with cell size 10x10 -> col = 20/10 + 1 = 3, row = 30/10 + 1 = 4
    assert!(output.starts_with("\x1b[4;3H"), "Expected cursor at 4;3H, got: {}", &output[..20]);
    assert!(output.contains("\x1b_Ga=T,q=2,i=2,p=1,C=1,f=32,o=z,z=-2147483647"));
    assert!(output.contains("s=20,v=10,c=2,r=1"));
    assert!(!output.contains("\x1b_Ga=d,d=a\x1b\\"), "Partial placement should not clear images");
    assert!(!output.contains("\x1b_Ga=d,d=i,i=2\x1b\\"), "Partial placement should not clear subimages");

    // Extract payload and verify pixels
    let g_idx = output.find("\x1b_G").expect("kitty header");
    let semi_idx = output[g_idx..].find(';').expect("control terminator") + g_idx;
    let end_idx = output.rfind("\x1b\\").expect("escape terminator");
    let b64_payload = &output[semi_idx + 1..end_idx];
    let compressed = BASE64.decode(b64_payload).expect("valid base64");
    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).expect("valid zlib");

    assert_eq!(decompressed.len(), (20 * 10 * 4) as usize);
    assert_eq!(decompressed[0], 200 & 0xF8);
    assert_eq!(decompressed[1], 50 & 0xFC);
    assert_eq!(decompressed[2], 80 & 0xF8);
    assert_eq!(decompressed[3], 255);
}

#[test]
fn test_kitty_moving_damage_no_ghosting_full_frame() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 160u32;
    let height = 100u32;
    let rgba = vec![10u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Warmup frame
    encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, None, false, &mut output,
    );

    // Frame 1: Damage at (20, 30)
    let damage_1 = [DamageRect {
        x: 20,
        y: 30,
        width: 20,
        height: 10,
    }];
    encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_1), false, &mut output,
    );

    // Frame 2: Damage moves to (40, 50) (e.g. xeyes pupil moving or window dragging)
    let damage_2 = [DamageRect {
        x: 40,
        y: 50,
        width: 20,
        height: 10,
    }];

    let is_partial = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        16,
        10,
        10,
        10,
        Some(&damage_2),
        false,
        &mut output,
    );

    // Moving damage MUST NOT use partial placement to guarantee zero ghosting
    assert!(!is_partial, "Moving damage must fall back to full frame");
    assert!(output.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648"));
    assert!(!output.contains("i=2"), "Must not use subimage i=2 when moving to avoid ghosting");
}

#[test]
fn test_kitty_subimage_neutralized_by_transparent_dummy_not_delete() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 160u32;
    let height = 100u32;
    let rgba = vec![10u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Warmup frame
    encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, None, false, &mut output,
    );

    // Frame 1: Damage at (20, 30) establishes anchor
    let damage_1 = [DamageRect {
        x: 20,
        y: 30,
        width: 20,
        height: 10,
    }];
    encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_1), false, &mut output,
    );

    // Frame 2: Same damage at (20, 30) emits partial frame
    let is_partial = encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_1), false, &mut output,
    );
    assert!(is_partial, "Must be partial frame");

    // Frame 3: Damage moves to (40, 50). Subimage was active, now anchor is lost!
    let damage_2 = [DamageRect {
        x: 40,
        y: 50,
        width: 20,
        height: 10,
    }];
    let is_partial_3 = encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_2), false, &mut output,
    );
    assert!(!is_partial_3, "Must fall back to full frame");

    // Must neutralize active subimage with 1x1 transparent dummy pixel, NEVER a=d
    assert!(output.contains(";AAAAAA==\x1b\\"), "Must emit 1x1 transparent dummy pixel");
    assert!(!output.contains("\x1b_Ga=d"), "Must NEVER emit a=d");
    assert!(output.contains("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648"));
}

#[test]
fn test_kitty_partial_placement_fallback_on_large_damage() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 100u32;
    let height = 100u32;
    let rgba = vec![128u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Warmup full frame
    encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, None, false, &mut output,
    );
    assert!(output.starts_with("\x1b[H"));

    // Large damage (> 75% of the screen, e.g. 90x90 = 8100 out of 10000 pixels)
    let large_damage = [DamageRect {
        x: 5,
        y: 5,
        width: 90,
        height: 90,
    }];

    let is_partial = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        10,
        10,
        10,
        10,
        Some(&large_damage),
        false,
        &mut output,
    );

    // Large damage must fall back to full-screen frame
    assert!(!is_partial);
    assert!(output.contains("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648,s=100,v=100,c=10,r=10"));
}

#[test]
fn test_kitty_partial_placement_periodic_consolidation() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 100u32;
    let height = 100u32;
    let rgba = vec![64u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Initial frame
    encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, None, false, &mut output,
    );

    let damage = [DamageRect {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    }];

    // Warmup 1 frame to establish stationary bbox
    let _ = encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, Some(&damage), false, &mut output,
    );

    // Run 60 partial frames
    for _ in 0..60 {
        let is_partial = encoder.encode_frame_into(
            &rgba,
            width,
            height,
            10,
            10,
            10,
            10,
            Some(&damage),
            false,
            &mut output,
        );
        assert!(is_partial);
        assert!(output.starts_with("\x1b[1;1H"), "Should be partial placement");
        assert!(!output.contains("\x1b_Ga=d,d=a\x1b\\"));
        assert!(!output.contains("\x1b_Ga=d,d=i,i=2\x1b\\"));
    }

    // 61st frame must trigger consolidation: delete prior subimage and send full frame
    let is_partial_61 = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        10,
        10,
        10,
        10,
        Some(&damage),
        false,
        &mut output,
    );
    assert!(!is_partial_61);
    assert!(output.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1"), "61st frame should consolidate with full frame and home cursor");
    assert!(!output.contains("\x1b_Ga=d"), "Must never issue a=d to prevent terminal emulator crashes");
    assert!(output.contains("s=100,v=100,c=10,r=10"));
}

#[test]
fn test_kitty_default_mode_fullscreen_only() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 100u32;
    let height = 100u32;
    let rgba = vec![64u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    let mut output = String::new();

    // Default encoder must have enable_partial = false
    assert!(!encoder.enable_partial());

    // Initial frame
    encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, None, false, &mut output,
    );

    let damage = [DamageRect {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    }];

    // Even with small, stationary damage, default must emit full frame i=1, p=1
    let is_partial = encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, Some(&damage), false, &mut output,
    );
    assert!(!is_partial, "Default mode must always emit full frame");
    assert!(output.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1"));
    assert!(!output.contains("i=2"));
}

#[test]
fn test_kitty_uncompressible_and_varying_sizes() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let mut encoder = KittyStreamEncoder::new();
    let mut output = String::new();

    let mut rng: u64 = 0x123456789abcdef0;
    let mut next_u8 = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng >> 33) as u8
    };

    for &(w, h) in &[(1, 1), (10, 10), (13, 17), (100, 100), (320, 240), (800, 600)] {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for b in &mut rgba {
            *b = next_u8();
        }
        encoder.encode_frame_into(
            &rgba, w, h, 80, 24, 10, 20, None, false, &mut output,
        );
        assert!(!output.is_empty());

        let damage = [DamageRect {
            x: 1,
            y: 1,
            width: (w / 3).max(1),
            height: (h / 3).max(1),
        }];
        encoder.encode_frame_into(
            &rgba, w, h, 80, 24, 10, 20, Some(&damage), false, &mut output,
        );
        assert!(!output.is_empty());
    }
}

#[test]
fn test_kitty_gui_pattern_compression_efficiency() {
    use termway::protocol::KittyStreamEncoder;

    let mut encoder = KittyStreamEncoder::new();
    let mut output = String::new();

    let w = 800;
    let h = 600;
    let mut rgba = vec![0x28u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y * w + x) * 4) as usize;
            rgba[idx + 1] = 0x2c;
            rgba[idx + 2] = 0x34;
            rgba[idx + 3] = 0xff;
            if x >= 100 && x <= 700 && y >= 100 && y <= 500 {
                rgba[idx] = 0x1e;
                rgba[idx + 1] = 0x1e;
                rgba[idx + 2] = 0x1e;
            }
        }
    }

    encoder.encode_frame_into(
        &rgba, w, h, 80, 24, 10, 20, None, false, &mut output,
    );
    // 800x600 uncompressed is 1.92MB. Level 2 dynamic Huffman compression should compress this to under 25KB.
    assert!(output.len() < 25_000, "Encoded output length {} exceeded 25,000 bytes", output.len());
    assert!(output.contains("s=800,v=600,c=80,r=24"));
}

#[test]
fn test_quantization_compression_comparison() {
    use flate2::{Compress, Compression, FlushCompress};

    // Simulate 1080x720 video frame with gradients and low-amplitude noise
    let w = 1080;
    let h = 720;
    let mut raw = vec![0u8; w * h * 4];
    let mut seed = 123456789u32;
    let mut lcg = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 24) as u8
    };

    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * 4;
            // Video gradient base + sensor noise (+-3)
            let noise = (lcg() % 7) as i16 - 3;
            let r = ((x * 255 / w) as i16 + noise).clamp(0, 255) as u8;
            let g = ((y * 255 / h) as i16 + noise).clamp(0, 255) as u8;
            let b = (((x + y) * 128 / (w + h)) as i16 + noise).clamp(0, 255) as u8;
            raw[idx] = r;
            raw[idx + 1] = g;
            raw[idx + 2] = b;
            raw[idx + 3] = 255;
        }
    }

    // Unmasked compression
    let mut comp1 = Compress::new(Compression::new(2), true);
    let mut out1 = vec![0u8; raw.len()];
    comp1.compress(&raw, &mut out1, FlushCompress::Finish).unwrap();
    let size_raw = comp1.total_out() as usize;

    // Masked RGB565: R & 0xF8, G & 0xFC, B & 0xF8, A & 0xFF
    let mut masked_565 = raw.clone();
    for p in masked_565.chunks_exact_mut(4) {
        p[0] &= 0xF8;
        p[1] &= 0xFC;
        p[2] &= 0xF8;
    }
    let mut comp2 = Compress::new(Compression::new(2), true);
    let mut out2 = vec![0u8; raw.len()];
    comp2.compress(&masked_565, &mut out2, FlushCompress::Finish).unwrap();
    let size_565 = comp2.total_out() as usize;

    // Masked RGB555: R & 0xF8, G & 0xF8, B & 0xF8, A & 0xFF
    let mut masked_555 = raw.clone();
    for p in masked_555.chunks_exact_mut(4) {
        p[0] &= 0xF8;
        p[1] &= 0xF8;
        p[2] &= 0xF8;
    }
    let mut comp3 = Compress::new(Compression::new(2), true);
    let mut out3 = vec![0u8; raw.len()];
    comp3.compress(&masked_555, &mut out3, FlushCompress::Finish).unwrap();
    let size_555 = comp3.total_out() as usize;

    eprintln!(
        "\n[QUANTIZATION BENCHMARK] 1080x720 video frame:\n  Raw RGBA32:   {} KB\n  RGB565 Mask:  {} KB ({:+.1}%)\n  RGB555 Mask:  {} KB ({:+.1}%)",
        size_raw / 1024,
        size_565 / 1024,
        ((size_565 as f64 - size_raw as f64) / size_raw as f64) * 100.0,
        size_555 / 1024,
        ((size_555 as f64 - size_raw as f64) / size_raw as f64) * 100.0,
    );
}

