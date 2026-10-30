//! Guards the committed application icon: it must be the official DeepSeek
//! blue whale, not the previous custom glyph or the dark DSH bundle icon.

use std::collections::HashMap;

#[test]
fn application_icon_is_the_blue_whale() {
    let bytes = include_bytes!("../assets/app/icon.png");
    let image = image::load_from_memory(bytes)
        .expect("assets/app/icon.png decodes")
        .into_rgba8();
    assert_eq!(image.dimensions(), (1024, 1024));

    let mut opaque: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let mut transparent = 0usize;
    for pixel in image.pixels() {
        if pixel[3] < 16 {
            transparent += 1;
        } else if pixel[3] > 200 {
            *opaque.entry((pixel[0], pixel[1], pixel[2])).or_insert(0) += 1;
        }
    }
    assert!(
        transparent > 100_000,
        "the whale icon must have a transparent background ({transparent} transparent pixels)"
    );
    let dominant = opaque
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(colour, _)| colour)
        .expect("opaque whale pixels");
    // DeepSeek brand blue used by the official whale SVG (#4D6BFE).
    assert_eq!(dominant, (0x4D, 0x6B, 0xFE));
}
