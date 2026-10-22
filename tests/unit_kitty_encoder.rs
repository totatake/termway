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

    // Frame 1: Initial damage bounding box immediately emits partial placement
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
    assert!(is_partial_1, "Initial damage should immediately emit partial placement");
    assert!(output.starts_with("\x1b[4;3H"), "Expected cursor at 4;3H, got: {}", &output[..20]);
    assert!(output.contains("\x1b_Ga=T,q=2,i=2,p=1,C=1,f=32,o=z,z=-2147483647"));
    assert!(output.contains("s=20,v=10,c=2,r=1"));
    assert!(!output.contains("\x1b_Ga=d,d=a\x1b\\"), "Partial placement should not clear images");
    assert!(!output.contains("\x1b_Ga=d,d=i,i=2\x1b\\"), "Partial placement should not clear subimages");

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
fn test_kitty_moving_damage_cumulative_bbox_and_adaptive_rebase() {
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
    let is_p1 = encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_1), false, &mut output,
    );
    assert!(is_p1, "Initial damage should emit partial placement");

    // Frame 2: Damage moves nearby to (25, 30) (e.g. xeyes pupil moving within eye)
    let damage_2 = [DamageRect {
        x: 25,
        y: 30,
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

    // Moving damage within bounds uses cumulative bounding box partial placement without ghosting
    assert!(is_partial, "Nearby moving damage should use partial placement");
    assert!(output.contains("i=2"), "Subimage i=2 must be used for partial placement");

    // Frame 3: Damage jumps far away across screen (e.g. cursor moves to 140, 80)
    let damage_far = [DamageRect {
        x: 140,
        y: 80,
        width: 10,
        height: 10,
    }];
    let is_partial_far = encoder.encode_frame_into(
        &rgba,
        width,
        height,
        16,
        10,
        10,
        10,
        Some(&damage_far),
        false,
        &mut output,
    );
    // Sparse inflation triggers adaptive rebase (full frame i=1)
    assert!(!is_partial_far, "Sparse inflated damage must trigger adaptive full frame rebase");
    assert!(output.contains("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1,f=32,o=z,z=-2147483648"));
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
    let is_partial = encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_1), false, &mut output,
    );
    assert!(is_partial, "Must be partial frame");

    // Frame 2: Damage jumps far away causing sparse inflation -> triggers fallback rebase
    let damage_far = [DamageRect {
        x: 140,
        y: 80,
        width: 10,
        height: 10,
    }];
    let is_partial_2 = encoder.encode_frame_into(
        &rgba, width, height, 16, 10, 10, 10, Some(&damage_far), false, &mut output,
    );
    assert!(!is_partial_2, "Must fall back to full frame");

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
fn test_kitty_partial_placement_unlimited_stationary_streaming() {
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

    // Run 100 partial frames: stationary video streaming continues indefinitely without 60-frame consolidation drop
    for frame in 1..=100 {
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
        assert!(is_partial, "Frame {} should maintain partial streaming", frame);
        assert!(output.starts_with("\x1b[1;1H"), "Should be partial placement");
        assert!(!output.contains("\x1b_Ga=d,d=a\x1b\\"));
        assert!(!output.contains("\x1b_Ga=d,d=i,i=2\x1b\\"));
    }
}

#[test]
fn test_kitty_default_mode_and_opt_out() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let width = 100u32;
    let height = 100u32;
    let rgba = vec![64u8; (width * height * 4) as usize];
    let mut encoder = KittyStreamEncoder::new();
    let mut output = String::new();

    // Default encoder must have enable_partial = true
    assert!(encoder.enable_partial());

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

    // By default, small damage emits partial placement
    let is_partial = encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, Some(&damage), false, &mut output,
    );
    assert!(is_partial, "Default mode should emit partial placement for small damage");
    assert!(output.contains("i=2"));

    // When opt-out via set_enable_partial(false), must emit full frame i=1, p=1
    encoder.set_enable_partial(false);
    assert!(!encoder.enable_partial());
    let is_partial_opt_out = encoder.encode_frame_into(
        &rgba, width, height, 10, 10, 10, 10, Some(&damage), false, &mut output,
    );
    assert!(!is_partial_opt_out, "Opted-out mode must always emit full frame");
    assert!(output.contains("\x1b[H\x1b_Ga=T,q=2,i=1,p=1,C=1"));
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

#[test]
fn test_kitty_resize_forces_fullscreen_rebase() {
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let mut encoder = KittyStreamEncoder::new();
    let mut output = String::new();

    // Frame 1 at 100x100
    let w1 = 100u32;
    let h1 = 100u32;
    let rgba1 = vec![50u8; (w1 * h1 * 4) as usize];
    encoder.encode_frame_into(
        &rgba1, w1, h1, 10, 10, 10, 10, None, false, &mut output,
    );

    // Small damage at 100x100 -> emits partial
    let dmg = [DamageRect {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    }];
    let is_p = encoder.encode_frame_into(
        &rgba1, w1, h1, 10, 10, 10, 10, Some(&dmg), false, &mut output,
    );
    assert!(is_p, "Small damage on same resolution should emit partial");

    // Frame 3: Window resized to 200x150, but caller passes small damage
    let w2 = 200u32;
    let h2 = 150u32;
    let rgba2 = vec![80u8; (w2 * h2 * 4) as usize];
    let is_p_resized = encoder.encode_frame_into(
        &rgba2, w2, h2, 20, 15, 10, 10, Some(&dmg), false, &mut output,
    );
    assert!(
        !is_p_resized,
        "Resizing must reject partial placement and force fullscreen rebase"
    );
    assert!(
        output.contains("s=200,v=150,c=20,r=15"),
        "Fullscreen sequence must reflect new dimensions"
    );
}

#[test]
fn test_kitty_jpeg_f101_fullscreen_and_partial_placement() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    encoder.set_use_jpeg(true);
    assert!(encoder.use_jpeg());

    let w = 160u32;
    let h = 100u32;
    let rgba = vec![120u8; (w * h * 4) as usize];
    let mut output = String::new();

    // 1. Initial fullscreen frame must use f=101 without o=z
    encoder.encode_frame_into(
        &rgba, w, h, 16, 10, 10, 10, None, false, &mut output,
    );
    assert!(output.contains("f=101"), "Fullscreen header must use f=101");
    assert!(!output.contains("o=z"), "JPEG mode must not specify o=z");
    assert!(output.contains("i=1,p=1"));

    // Extract payload and verify JPEG SOI and EOI
    let semi_idx = output.find(';').unwrap();
    let esc_idx = output.find("\x1b\\").unwrap();
    let b64 = &output[semi_idx + 1..esc_idx];
    let jpeg_bytes = BASE64.decode(b64).expect("valid base64");
    assert_eq!(jpeg_bytes[0], 0xFF);
    assert_eq!(jpeg_bytes[1], 0xD8); // SOI
    assert_eq!(jpeg_bytes[jpeg_bytes.len() - 2], 0xFF);
    assert_eq!(jpeg_bytes[jpeg_bytes.len() - 1], 0xD9); // EOI

    // 2. Partial damage must use f=101 for subimage i=2
    let dmg = [DamageRect {
        x: 20,
        y: 30,
        width: 20,
        height: 10,
    }];
    let is_p = encoder.encode_frame_into(
        &rgba, w, h, 16, 10, 10, 10, Some(&dmg), false, &mut output,
    );
    assert!(is_p, "Partial frame should emit subimage");
    assert!(output.contains("f=101"), "Subimage must use f=101");
    assert!(!output.contains("o=z"), "Subimage must not specify o=z");
    assert!(output.contains("i=2,p=1"));

    // 3. Fallback / clear_placements must preserve f=32 transparent dummy pixel
    encoder.encode_frame_into(
        &rgba, w, h, 16, 10, 10, 10, None, false, &mut output,
    );
    assert!(
        output.contains("f=32,s=1,v=1,c=1,r=1;AAAAAA==\x1b\\"),
        "Dummy pixel cleanup must remain f=32 for alpha transparency"
    );
}

#[test]
fn test_kitty_jpeg_quality_configuration() {
    use termway::protocol::KittyStreamEncoder;

    let mut encoder = KittyStreamEncoder::new();
    assert_eq!(encoder.jpeg_quality(), 75); // default

    encoder.set_jpeg_quality(95);
    assert_eq!(encoder.jpeg_quality(), 95);

    let w = 80u32;
    let h = 60u32;
    let rgba = vec![140u8; (w * h * 4) as usize];
    let mut out_high = String::new();
    encoder.set_use_jpeg(true);
    encoder.encode_frame_into(
        &rgba, w, h, 8, 6, 10, 10, None, false, &mut out_high,
    );

    encoder.set_jpeg_quality(15);
    assert_eq!(encoder.jpeg_quality(), 15);
    let mut out_low = String::new();
    encoder.encode_frame_into(
        &rgba, w, h, 8, 6, 10, 10, None, false, &mut out_low,
    );

    // Higher quality JPEG has less quantization and should be larger or equal
    assert!(out_high.len() >= out_low.len());
}

#[test]
fn test_kitty_pixel_level_zero_ghosting_simulation() {
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use termway::display::DamageRect;
    use termway::protocol::KittyStreamEncoder;

    struct SimKittyTerminal {
        width: u32,
        height: u32,
        cell_w: u32,
        cell_h: u32,
        base_canvas: Vec<u8>,
        overlay_img: Option<(u32, u32, u32, u32, Vec<u8>)>,
    }

    impl SimKittyTerminal {
        fn new(width: u32, height: u32, cell_w: u32, cell_h: u32) -> Self {
            Self {
                width,
                height,
                cell_w,
                cell_h,
                base_canvas: vec![0u8; (width * height * 4) as usize],
                overlay_img: None,
            }
        }

        fn apply_kitty_seq(&mut self, seq: &str) {
            let mut cur = 0;
            while let Some(g_rel) = seq[cur..].find("\x1b_G") {
                let g_start = cur + g_rel;
                let semi = match seq[g_start..].find(';') {
                    Some(s) => g_start + s,
                    None => break,
                };
                let end = match seq[semi..].find("\x1b\\") {
                    Some(e) => semi + e,
                    None => break,
                };
                let header = &seq[g_start + 3..semi];
                let chunk_payload = &seq[semi + 1..end];

                // Check cursor position before this image command (e.g. \x1b[{row};{col}H)
                let prefix = &seq[cur..g_start];
                let mut col = 1u32;
                let mut row = 1u32;
                if let Some(cup_start) = prefix.rfind("\x1b[") {
                    if let Some(cup_end) = prefix[cup_start..].find('H') {
                        let inner = &prefix[cup_start + 2..cup_start + cup_end];
                        let parts: Vec<&str> = inner.split(';').collect();
                        if parts.len() == 2 {
                            row = parts[0].parse().unwrap_or(1);
                            col = parts[1].parse().unwrap_or(1);
                        }
                    }
                }

                // Handle dummy 1x1 neutralization
                if chunk_payload == "AAAAAA==" {
                    self.overlay_img = None;
                    cur = end + 2;
                    continue;
                }

                let is_base = header.contains("i=1");
                let is_sub = header.contains("i=2");

                // If chunked m=1, gather all chunks of this image
                let mut payload = chunk_payload.to_string();
                let mut is_last = !header.contains("m=1");
                cur = end + 2;

                while !is_last {
                    if let Some(next_g) = seq[cur..].find("\x1b_G") {
                        let next_start = cur + next_g;
                        let next_semi = seq[next_start..].find(';').unwrap() + next_start;
                        let next_end = seq[next_semi..].find("\x1b\\").unwrap() + next_semi;
                        let next_header = &seq[next_start + 3..next_semi];
                        payload.push_str(&seq[next_semi + 1..next_end]);
                        is_last = !next_header.contains("m=1");
                        cur = next_end + 2;
                    } else {
                        break;
                    }
                }

                let comp = BASE64.decode(&payload).expect("valid base64");
                let decomp = miniz_oxide::inflate::decompress_to_vec_zlib(&comp).expect("valid zlib");

                if is_base {
                    assert_eq!(decomp.len(), (self.width * self.height * 4) as usize);
                    self.base_canvas.copy_from_slice(&decomp);
                } else if is_sub {
                    let mut sub_w = 0u32;
                    let mut sub_h = 0u32;
                    for part in header.split(',') {
                        if let Some(val) = part.strip_prefix("s=") {
                            sub_w = val.parse().unwrap();
                        } else if let Some(val) = part.strip_prefix("v=") {
                            sub_h = val.parse().unwrap();
                        }
                    }
                    let px_x = (col - 1) * self.cell_w;
                    let px_y = (row - 1) * self.cell_h;
                    assert_eq!(decomp.len(), (sub_w * sub_h * 4) as usize);
                    self.overlay_img = Some((px_x, px_y, sub_w, sub_h, decomp));
                }
            }
        }

        fn composite(&self) -> Vec<u8> {
            let mut result = self.base_canvas.clone();
            if let Some((ox, oy, ow, oh, ref odata)) = self.overlay_img {
                let stride = (self.width * 4) as usize;
                let o_stride = (ow * 4) as usize;
                for y in 0..oh {
                    let dy = oy + y;
                    if dy >= self.height {
                        break;
                    }
                    let src_row = &odata[(y as usize * o_stride)..((y + 1) as usize * o_stride)];
                    let dst_start = (dy as usize * stride) + (ox as usize * 4);
                    let dst_end = dst_start + (ow as usize * 4);
                    if dst_end <= result.len() {
                        result[dst_start..dst_end].copy_from_slice(src_row);
                    }
                }
            }
            result
        }
    }

    fn quantize_rgb565(raw: &[u8]) -> Vec<u8> {
        let mut out = raw.to_vec();
        for p in out.chunks_exact_mut(4) {
            p[0] &= 0xF8;
            p[1] &= 0xFC;
            p[2] &= 0xF8;
        }
        out
    }

    let width = 160u32;
    let height = 100u32;
    let cell_w = 10u32;
    let cell_h = 10u32;
    let term_cols = 16u16;
    let term_rows = 10u16;

    let mut sim = SimKittyTerminal::new(width, height, cell_w, cell_h);
    let mut encoder = KittyStreamEncoder::new();
    encoder.set_enable_partial(true);
    let mut output = String::new();

    // Frame 0: Solid green background (0, 180, 0, 255)
    let mut frame = vec![0u8; (width * height * 4) as usize];
    for p in frame.chunks_exact_mut(4) {
        p[0] = 0;
        p[1] = 180;
        p[2] = 0;
        p[3] = 255;
    }
    encoder.encode_frame_into(
        &frame, width, height, term_cols, term_rows, cell_w as u16, cell_h as u16, None, false, &mut output,
    );
    sim.apply_kitty_seq(&output);
    assert_eq!(sim.composite(), quantize_rgb565(&frame), "Frame 0 composite mismatch");

    // Frame 1: Pupil at (20..30, 30..40) painted black (0, 0, 0, 255)
    for y in 30..40 {
        for x in 20..30 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 0; frame[idx + 1] = 0; frame[idx + 2] = 0; frame[idx + 3] = 255;
        }
    }
    let dmg1 = [DamageRect { x: 20, y: 30, width: 10, height: 10 }];
    let is_p1 = encoder.encode_frame_into(
        &frame, width, height, term_cols, term_rows, cell_w as u16, cell_h as u16, Some(&dmg1), false, &mut output,
    );
    assert!(is_p1, "Frame 1 must be partial");
    sim.apply_kitty_seq(&output);
    assert_eq!(sim.composite(), quantize_rgb565(&frame), "Frame 1 zero-ghosting verification");

    // Frame 2: Pupil moves to (25..35, 30..40). Pos A (20..25) restored to green.
    for y in 30..40 {
        for x in 20..25 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 0; frame[idx + 1] = 180; frame[idx + 2] = 0; frame[idx + 3] = 255;
        }
        for x in 25..35 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 0; frame[idx + 1] = 0; frame[idx + 2] = 0; frame[idx + 3] = 255;
        }
    }
    let dmg2 = [DamageRect { x: 20, y: 30, width: 15, height: 10 }];
    let is_p2 = encoder.encode_frame_into(
        &frame, width, height, term_cols, term_rows, cell_w as u16, cell_h as u16, Some(&dmg2), false, &mut output,
    );
    assert!(is_p2, "Frame 2 must be partial");
    sim.apply_kitty_seq(&output);
    assert_eq!(
        sim.composite(),
        quantize_rgb565(&frame),
        "Frame 2 zero-ghosting: previous pupil location must be restored cleanly with zero ghosting"
    );

    // Frame 3: Pupil moves further to (28..38, 32..42). Pos B restored to green.
    for y in 30..40 {
        for x in 25..35 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 0; frame[idx + 1] = 180; frame[idx + 2] = 0; frame[idx + 3] = 255;
        }
    }
    for y in 32..42 {
        for x in 28..38 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 0; frame[idx + 1] = 0; frame[idx + 2] = 0; frame[idx + 3] = 255;
        }
    }
    let dmg3 = [DamageRect { x: 25, y: 30, width: 13, height: 12 }];
    let is_p3 = encoder.encode_frame_into(
        &frame, width, height, term_cols, term_rows, cell_w as u16, cell_h as u16, Some(&dmg3), false, &mut output,
    );
    assert!(is_p3, "Frame 3 must be partial");
    sim.apply_kitty_seq(&output);
    assert_eq!(
        sim.composite(),
        quantize_rgb565(&frame),
        "Frame 3 zero-ghosting: cumulative subimage must cleanly cover both past pupil footprints"
    );

    // Frame 4: Distant cursor appears at (140, 80). Sparse inflation triggers adaptive rebase.
    for y in 80..85 {
        for x in 140..145 {
            let idx = ((y * width + x) * 4) as usize;
            frame[idx] = 255; frame[idx + 1] = 255; frame[idx + 2] = 255; frame[idx + 3] = 255;
        }
    }
    let dmg4 = [DamageRect { x: 140, y: 80, width: 5, height: 5 }];
    let is_p4 = encoder.encode_frame_into(
        &frame, width, height, term_cols, term_rows, cell_w as u16, cell_h as u16, Some(&dmg4), false, &mut output,
    );
    assert!(!is_p4, "Frame 4 must trigger adaptive rebase to full frame");
    sim.apply_kitty_seq(&output);
    assert_eq!(
        sim.composite(),
        quantize_rgb565(&frame),
        "Frame 4 zero-ghosting: adaptive rebase must cleanly display full frame"
    );
}

