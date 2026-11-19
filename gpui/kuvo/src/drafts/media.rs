//! Procedural stand-in screenshots for the `show_file` drafts, built with `image` so every shot
//! is deterministic, fast, and reads as real product UI rather than flat colour washes.

use std::sync::Arc;

use gpui::{Image, ImageFormat};
use image::{Rgb, RgbImage};

/// A generated stand-in picture with its natural dimensions.
#[derive(Clone)]
pub struct SamplePicture {
    pub image: Arc<Image>,
    pub width: u32,
    pub height: u32,
}

/// The four stand-in pictures shared across all drafts so the options compare apples to apples.
#[derive(Clone)]
pub struct SampleMedia {
    pub pricing_hero: SamplePicture,
    pub checkout_light: SamplePicture,
    pub checkout_dark: SamplePicture,
    pub checkout_narrow: SamplePicture,
}

impl SampleMedia {
    pub fn generate() -> Self {
        Self {
            pricing_hero: encode(pricing_hero_image()),
            checkout_light: encode(checkout_desktop(false)),
            checkout_dark: encode(checkout_desktop(true)),
            checkout_narrow: encode(checkout_mobile()),
        }
    }
}

fn encode(img: RgbImage) -> SamplePicture {
    let (width, height) = (img.width(), img.height());
    let mut bytes = std::io::Cursor::new(Vec::new());
    img.write_to(&mut bytes, image::ImageFormat::Png).expect("encode stand-in PNG");
    SamplePicture {
        image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes.into_inner())),
        width,
        height,
    }
}

/// Draws a filled rounded rectangle with anti-aliased corner edges.
fn fill_round_rect(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, r: i32, color: [u8; 3]) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let r = r.min(w / 2).min(h / 2).max(0);
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(iw);
    let y1 = (y + h).min(ih);
    let rf = r as f32;
    for py in y0..y1 {
        for px in x0..x1 {
            let lx = px - x;
            let ly = py - y;
            let dx = if lx < r {
                r - lx
            } else if lx >= w - r {
                lx - (w - r - 1)
            } else {
                0
            };
            let dy = if ly < r {
                r - ly
            } else if ly >= h - r {
                ly - (h - r - 1)
            } else {
                0
            };
            if dx == 0 && dy == 0 {
                img.put_pixel(px as u32, py as u32, Rgb(color));
            } else {
                let dist = ((dx * dx + dy * dy) as f32).sqrt();
                if dist <= rf - 0.5 {
                    img.put_pixel(px as u32, py as u32, Rgb(color));
                } else if dist < rf + 0.5 {
                    let alpha = (rf + 0.5 - dist).clamp(0.0, 1.0);
                    let dst = img.get_pixel(px as u32, py as u32).0;
                    let blended = std::array::from_fn(|i| {
                        (dst[i] as f32 * (1.0 - alpha) + color[i] as f32 * alpha).round() as u8
                    });
                    img.put_pixel(px as u32, py as u32, Rgb(blended));
                }
            }
        }
    }
}

/// Draws a 1–2 px stroked rounded rectangle border by layering fill inside border.
fn bordered_round_rect(
    img: &mut RgbImage,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    border_w: i32,
    border: [u8; 3],
    fill: [u8; 3],
) {
    fill_round_rect(img, x, y, w, h, r, border);
    fill_round_rect(
        img,
        x + border_w,
        y + border_w,
        w - border_w * 2,
        h - border_w * 2,
        (r - border_w).max(0),
        fill,
    );
}

fn fill_circle(img: &mut RgbImage, cx: i32, cy: i32, r: i32, color: [u8; 3]) {
    fill_round_rect(img, cx - r, cy - r, r * 2, r * 2, r, color);
}

/// A crisp 16:10 pricing hero mockup (800×500): subtle browser top bar, headline, annual pill
/// toggle, and 3 pricing tier cards with the middle tier highlighted.
fn pricing_hero_image() -> RgbImage {
    let (w, h) = (800, 500);
    let mut img = RgbImage::from_fn(w, h, |x, y| {
        // Gentle cool-warm backdrop gradient so the page has depth.
        let t = y as f32 / h as f32;
        let s = x as f32 / w as f32;
        let r = (247.0 - t * 6.0 - s * 4.0) as u8;
        let g = (248.0 - t * 5.0 - s * 2.0) as u8;
        let b = (252.0 - t * 2.0) as u8;
        Rgb([r, g, b])
    });

    // Browser chrome strip.
    fill_round_rect(&mut img, 0, 0, 800, 38, 0, [255, 255, 255]);
    fill_round_rect(&mut img, 0, 38, 800, 1, 0, [232, 234, 240]);
    fill_circle(&mut img, 22, 19, 5, [238, 106, 95]);
    fill_circle(&mut img, 39, 19, 5, [244, 191, 79]);
    fill_circle(&mut img, 56, 19, 5, [97, 197, 84]);
    // URL pill.
    fill_round_rect(&mut img, 265, 9, 270, 20, 10, [242, 244, 248]);
    fill_round_rect(&mut img, 352, 17, 96, 4, 2, [175, 182, 194]);

    // Nav row inside page.
    fill_round_rect(&mut img, 56, 56, 22, 22, 6, [25, 28, 36]);
    fill_round_rect(&mut img, 88, 64, 54, 7, 3, [40, 44, 54]);
    for (i, nw) in [44, 48, 38].into_iter().enumerate() {
        fill_round_rect(&mut img, 320 + i as i32 * 64, 65, nw, 5, 2, [150, 156, 168]);
    }
    fill_round_rect(&mut img, 676, 56, 68, 24, 12, [25, 28, 36]);

    // Hero headline + subcopy bars.
    fill_round_rect(&mut img, 224, 106, 352, 18, 6, [26, 29, 38]);
    fill_round_rect(&mut img, 276, 134, 248, 12, 4, [132, 140, 154]);
    fill_round_rect(&mut img, 324, 154, 152, 8, 4, [166, 173, 186]);

    // Monthly / Annual toggle pill.
    bordered_round_rect(&mut img, 304, 178, 192, 32, 16, 1, [224, 228, 236], [255, 255, 255]);
    fill_round_rect(&mut img, 326, 191, 56, 6, 3, [140, 146, 160]);
    fill_round_rect(&mut img, 404, 182, 88, 24, 12, [10, 122, 255]);
    fill_round_rect(&mut img, 424, 191, 48, 6, 3, [255, 255, 255]);

    // Three pricing tier cards.
    let card_y = 230;
    let card_h = 236;
    let card_w = 216;
    let xs = [56, 292, 528];
    for (idx, cx) in xs.into_iter().enumerate() {
        let featured = idx == 1;
        let border = if featured { [10, 122, 255] } else { [226, 230, 238] };
        let bw = if featured { 2 } else { 1 };
        bordered_round_rect(&mut img, cx, card_y, card_w, card_h, 14, bw, border, [255, 255, 255]);

        // Tier badge + title.
        if featured {
            fill_round_rect(&mut img, cx + 18, card_y + 18, 64, 18, 9, [231, 242, 255]);
            fill_round_rect(&mut img, cx + 29, card_y + 25, 42, 4, 2, [10, 122, 255]);
        } else {
            fill_round_rect(&mut img, cx + 18, card_y + 22, 56, 8, 4, [152, 158, 170]);
        }

        // Price block.
        fill_round_rect(&mut img, cx + 18, card_y + 48, 76, 22, 5, [28, 31, 40]);
        fill_round_rect(&mut img, cx + 102, card_y + 59, 34, 7, 3, [170, 176, 188]);

        // CTA button.
        let cta_bg = if featured { [10, 122, 255] } else { [242, 244, 248] };
        let cta_fg = if featured { [255, 255, 255] } else { [54, 58, 68] };
        fill_round_rect(&mut img, cx + 18, card_y + 84, card_w - 36, 30, 8, cta_bg);
        fill_round_rect(&mut img, cx + 64, card_y + 96, 88, 6, 3, cta_fg);

        // Divider.
        fill_round_rect(&mut img, cx + 18, card_y + 128, card_w - 36, 1, 0, [236, 238, 244]);

        // Feature list rows.
        for row in 0..4 {
            let ry = card_y + 144 + row * 20;
            let dot = if featured { [10, 122, 255] } else { [46, 158, 91] };
            fill_circle(&mut img, cx + 24, ry + 4, 4, dot);
            let lw = 112 + ((idx * 17 + row as usize * 23) % 44) as i32;
            fill_round_rect(&mut img, cx + 38, ry + 1, lw, 6, 3, [142, 148, 162]);
        }
    }

    img
}

/// Checkout screen in light or dark mode (720×500).
fn checkout_desktop(dark: bool) -> RgbImage {
    let (w, h) = (720, 500);
    let bg = if dark { [18, 19, 23] } else { [245, 246, 249] };
    let card = if dark { [28, 30, 36] } else { [255, 255, 255] };
    let well = if dark { [22, 24, 29] } else { [244, 245, 248] };
    let border = if dark { [48, 52, 62] } else { [226, 229, 236] };
    let text = if dark { [236, 238, 244] } else { [34, 37, 46] };
    let muted = if dark { [126, 132, 148] } else { [152, 158, 172] };
    let accent = if dark { [46, 139, 255] } else { [10, 122, 255] };

    let mut img = RgbImage::from_pixel(w, h, Rgb(bg));

    // Top bar.
    fill_round_rect(&mut img, 0, 0, 720, 44, 0, card);
    fill_round_rect(&mut img, 0, 44, 720, 1, 0, border);
    fill_round_rect(&mut img, 28, 14, 18, 18, 5, accent);
    fill_round_rect(&mut img, 56, 19, 92, 8, 4, text);
    fill_round_rect(&mut img, 616, 16, 76, 14, 7, well);

    // Left column: order summary card.
    bordered_round_rect(&mut img, 28, 68, 296, 396, 14, 1, border, card);
    fill_round_rect(&mut img, 50, 92, 118, 10, 4, text);
    fill_round_rect(&mut img, 50, 112, 164, 6, 3, muted);

    // Plan swatch row.
    bordered_round_rect(&mut img, 50, 138, 252, 74, 10, 1, border, well);
    fill_round_rect(&mut img, 64, 154, 42, 42, 9, accent);
    fill_round_rect(&mut img, 118, 160, 102, 9, 4, text);
    fill_round_rect(&mut img, 118, 178, 136, 6, 3, muted);

    // Line items.
    for row in 0..3 {
        let ry = 236 + row * 34;
        fill_round_rect(&mut img, 50, ry, 124 - row * 12, 7, 3, muted);
        fill_round_rect(&mut img, 254, ry, 48, 7, 3, text);
        fill_round_rect(&mut img, 50, ry + 18, 252, 1, 0, border);
    }

    // Total row.
    fill_round_rect(&mut img, 50, 356, 76, 11, 4, text);
    fill_round_rect(&mut img, 222, 352, 80, 18, 5, text);
    // Security note pill.
    fill_round_rect(&mut img, 50, 404, 252, 36, 9, well);
    fill_circle(&mut img, 68, 422, 6, [46, 158, 91]);
    fill_round_rect(&mut img, 84, 419, 178, 6, 3, muted);

    // Right column: payment form card.
    bordered_round_rect(&mut img, 344, 68, 348, 396, 14, 1, border, card);
    fill_round_rect(&mut img, 368, 92, 134, 10, 4, text);

    // Quick pay button.
    fill_round_rect(&mut img, 368, 120, 300, 36, 9, text);
    fill_round_rect(&mut img, 474, 135, 88, 7, 3, card);

    // Input fields.
    for (idx, label_w) in [72, 96, 64].into_iter().enumerate() {
        let fy = 180 + idx as i32 * 64;
        fill_round_rect(&mut img, 368, fy, label_w, 6, 3, muted);
        bordered_round_rect(&mut img, 368, fy + 14, 300, 34, 8, 1, border, well);
        fill_round_rect(&mut img, 382, fy + 28, 132 + idx as i32 * 18, 6, 3, muted);
    }

    // Primary blue submit button.
    fill_round_rect(&mut img, 368, 384, 300, 42, 10, accent);
    fill_round_rect(&mut img, 468, 401, 100, 8, 4, [255, 255, 255]);

    img
}

/// Narrow 390px mobile checkout sheet (portrait 420×560 — intentionally different aspect ratio).
fn checkout_mobile() -> RgbImage {
    let (w, h) = (420, 560);
    let bg = [245, 246, 249];
    let card = [255, 255, 255];
    let well = [243, 244, 248];
    let border = [226, 229, 236];
    let text = [34, 37, 46];
    let muted = [150, 156, 170];
    let accent = [10, 122, 255];

    let mut img = RgbImage::from_pixel(w, h, Rgb(bg));

    // Mobile status & header bar.
    fill_round_rect(&mut img, 0, 0, 420, 54, 0, card);
    fill_round_rect(&mut img, 0, 54, 420, 1, 0, border);
    fill_round_rect(&mut img, 170, 10, 80, 6, 3, [215, 218, 226]);
    fill_round_rect(&mut img, 24, 28, 18, 16, 4, accent);
    fill_round_rect(&mut img, 52, 32, 118, 8, 4, text);
    fill_round_rect(&mut img, 344, 30, 52, 12, 6, well);

    // Compact summary card.
    bordered_round_rect(&mut img, 20, 72, 380, 92, 14, 1, border, card);
    fill_round_rect(&mut img, 36, 90, 48, 48, 10, accent);
    fill_round_rect(&mut img, 98, 96, 132, 9, 4, text);
    fill_round_rect(&mut img, 98, 114, 168, 6, 3, muted);
    fill_round_rect(&mut img, 314, 102, 68, 14, 5, text);

    // Form card.
    bordered_round_rect(&mut img, 20, 180, 380, 356, 14, 1, border, card);
    fill_round_rect(&mut img, 36, 200, 348, 40, 10, text);
    fill_round_rect(&mut img, 160, 217, 100, 7, 3, card);

    for (idx, lw) in [78, 104, 68].into_iter().enumerate() {
        let fy = 262 + idx as i32 * 64;
        fill_round_rect(&mut img, 36, fy, lw, 6, 3, muted);
        bordered_round_rect(&mut img, 36, fy + 14, 348, 36, 9, 1, border, well);
        fill_round_rect(&mut img, 50, fy + 29, 152 + idx as i32 * 14, 6, 3, muted);
    }

    // Sticky CTA button.
    fill_round_rect(&mut img, 36, 472, 348, 44, 11, accent);
    fill_round_rect(&mut img, 154, 490, 112, 8, 4, [255, 255, 255]);

    img
}
