//! Pictures attached to a message for another computer go as the phone sends them: a Retina
//! screenshot (a 2560×1640 PNG of several MB) as a JPEG at most 2048 px on its longest side, a
//! small picture as it is.

use base64::Engine as _;
use kuvo_box::parts::{PICTURE_KEPT_MOST, PICTURE_SIDE_MOST, carried};

/// A 2560×1640 PNG, noisy in its first `noisy` rows and flat below (a screenshot's weight).
fn screenshot(path: &std::path::Path, noisy: u32) {
    let mut seed: u32 = 0x9e37_79b9;
    let picture = image::RgbImage::from_fn(2560, 1640, |_, y| {
        if y < noisy {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
        } else {
            image::Rgb([40, 44, 52])
        }
    });
    picture.save_with_format(path, image::ImageFormat::Png).unwrap();
}

fn decoded(uri: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(uri.split_once(',').unwrap().1).unwrap()
}

#[test]
fn a_retina_screenshot_goes_to_another_computer_as_a_smaller_jpeg_and_a_small_picture_as_it_is() {
    let folder = tempfile::tempdir().unwrap();
    let big = folder.path().join("Pasted Image 2026-11-16 at 18.48.40.png");
    screenshot(&big, 700);
    let size = std::fs::metadata(&big).unwrap().len() as usize;
    assert!(size > PICTURE_KEPT_MOST, "a screenshot's weight: {size}");
    let small = folder.path().join("small.png");
    screenshot(&small, 20);
    assert!((std::fs::metadata(&small).unwrap().len() as usize) < PICTURE_KEPT_MOST);

    let files = carried(&[big, small.clone()], false, &|_| false).unwrap();
    // The screenshot: a JPEG, named so, no wider than 2048 px, a small part of what it was.
    assert!(files[0].uri.starts_with("data:image/jpeg;base64,"));
    assert_eq!(files[0].name.as_deref(), Some("Pasted Image 2026-11-16 at 18.48.40.jpg"));
    let jpeg = decoded(&files[0].uri);
    let picture = image::load_from_memory(&jpeg).unwrap();
    assert_eq!(picture.width(), PICTURE_SIDE_MOST);
    assert!((1311..=1312).contains(&picture.height()), "its shape kept: {}", picture.height());
    assert!(jpeg.len() < size / 2, "{} of {size}", jpeg.len());
    // The small one: as it was, byte for byte.
    assert!(files[1].uri.starts_with("data:image/png;base64,"));
    assert_eq!(decoded(&files[1].uri), std::fs::read(&small).unwrap());
}

#[test]
fn what_a_big_picture_leaves_see_through_arrives_white_not_black() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("window shot.png");
    // A window's screenshot: noisy where the window is, see-through around it (its shadow's room).
    let mut seed: u32 = 0x1234_5678;
    let picture = image::RgbaImage::from_fn(2560, 1640, |_, y| {
        if y < 900 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgba([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    picture.save_with_format(&path, image::ImageFormat::Png).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() as usize > PICTURE_KEPT_MOST);
    let files = carried(&[path], false, &|_| false).unwrap();
    let sent = image::load_from_memory(&decoded(&files[0].uri)).unwrap().to_rgb8();
    let corner = sent.get_pixel(10, sent.height() - 10).0;
    assert!(corner.iter().all(|channel| *channel > 240), "white, not black: {corner:?}");
}
