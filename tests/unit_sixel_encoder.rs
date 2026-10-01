use termway::protocol::SixelEncoder;

#[test]
fn test_sixel_blank_image() {
    let rgba = vec![0u8; 4 * 4 * 4]; // 4x4 completely transparent/black image
    let encoded = SixelEncoder::encode_fullscreen_frame(&rgba, 4, 4);

    // It should start with DCS q
    assert!(encoded.starts_with("\x1bPq"));
    // It should define color 0
    assert!(encoded.contains("#0;2;0;0;0"));
    // It should have some blank RLE `!4?` (4 empty pixels)
    assert!(encoded.contains("!4N"));
    // It should end with ST
    assert!(encoded.ends_with("\x1b\\"));
}

#[test]
fn test_sixel_rle() {
    let mut rgba = vec![0u8; 10 * 1 * 4]; // 10x1 image
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
