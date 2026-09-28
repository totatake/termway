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
    assert!(encoded.starts_with("\x1b_Ga=T,f=32,s=2,v=2"));
    assert!(encoded.ends_with("\x1b\\"));
    assert!(encoded.contains(",m=0;"));
}

#[test]
fn test_encode_rgba_subimage() {
    // 4x4 image
    let mut full = vec![0u8; 4 * 4 * 4];
    // Fill pixel (1, 1) with green
    let idx = (1 * 4 + 1) * 4;
    full[idx + 1] = 255;
    full[idx + 3] = 255;

    // Extract 2x2 subimage starting at (1, 1)
    let encoded = KittyEncoder::encode_rgba_subimage(&full, 4, 4, 1, 1, 2, 2, Some(5), Some(10));
    assert!(encoded.starts_with("\x1b_Ga=T,f=32,s=2,v=2,X=5,Y=10,m=0;"));
}

#[test]
fn test_chunked_transfer_for_large_image() {
    // Large image > 4096 base64 chars
    // 64x64 pixels * 4 bytes = 16384 bytes -> base64 is ~21848 bytes
    let large_rgba = vec![128u8; 64 * 64 * 4];
    let encoded = KittyEncoder::encode_rgba_image(&large_rgba, 64, 64, None, None);

    // Should have intermediate m=1; chunks and ending m=0; chunk
    assert!(encoded.contains("m=1;"));
    assert!(encoded.contains("m=0;"));
}
