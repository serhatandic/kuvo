//! Every bundled logo is drawn at one standard size: measured from the pixels its file draws, not
//! from the file's numbers, so artwork that's padded differently can't look bigger or smaller.

#[path = "support/marks_measure.rs"]
mod marks_measure;

use marks_measure::{LONGEST, OPTICAL, frame_side, measure, optical_side};

fn bundled() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/marks");
    let mut files = Vec::new();
    for folder in [root.clone(), root.join("providers")] {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "svg") {
                files.push((path.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

/// Each logo's visible shape, in the square its file frames, is centred, never bigger than the
/// frame, and as big to the eye as every other: its box fills the frame, or (a round or thin mark
/// that doesn't) its optical size is the standard's. So the same slot gives every logo one size.
#[test]
fn every_logo_is_the_same_optical_size() {
    let files = bundled();
    assert!(files.len() >= 70, "the marks are bundled: {}", files.len());
    for (name, svg) in files {
        let ink = measure(&svg, 480);
        let [vx, vy, vw, vh] = ink.view;
        assert!((vw - vh).abs() < 0.01, "{name}: the frame is a square, {vw} by {vh}");
        let (cx, cy) = ((ink.x + ink.w / 2. - vx) / vw - 0.5, (ink.y + ink.h / 2. - vy) / vh - 0.5);
        assert!(cx.abs() < 0.02 && cy.abs() < 0.02, "{name}: centred in its slot, off by {cx:.3}, {cy:.3}");
        let longest = ink.w.max(ink.h) / vw;
        let optical = optical_side(&ink) / vw;
        assert!(longest <= LONGEST + 0.02, "{name}: its box fits the slot, {longest:.3}");
        assert!(longest >= 0.78, "{name}: not smaller than the others, its longest side is {longest:.3}");
        let full = longest >= LONGEST - 0.03;
        let standard = (optical - OPTICAL).abs() <= 0.03;
        assert!(full || standard, "{name}: fills its slot ({longest:.3}) or is the standard's optical size ({optical:.3} against {OPTICAL})");
        // (AMD's is a strip four times as wide as it is tall: full width, and thin by its artwork.)
        assert!(optical <= OPTICAL + 0.03 && optical >= 0.4, "{name}: optical size {optical:.3}");
    }
}

/// The frame a logo is given is what the standard works out from its artwork: a new logo dropped
/// in the folder fails here until `cargo run -p kuvo_ui --example normalize_marks` has framed it.
#[test]
fn the_frames_are_the_standards_own() {
    for (name, svg) in bundled() {
        let ink = measure(&svg, 480);
        let side = frame_side(&ink);
        assert!((ink.view[2] - side).abs() < side * 0.03, "{name}: framed {} but the standard is {side:.2}", ink.view[2]);
    }
}
