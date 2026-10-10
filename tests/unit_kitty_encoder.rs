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
    assert!(encoded.starts_with("\x1b[H\x1b_Ga=T,q=2,i=1,C=1,f=32,o=z,z=-1,s=32,v=32,c=80,r=24"));
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
    assert!(encoded.starts_with("\x1b[H\x1b_Ga=T,q=2,i=42,C=1,f=32,o=z,z=-1,s=256,v=256,c=120,r=40"));
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
    assert_eq!(decompressed, rgba);
}
