//! Sets every bundled mark's frame to Kuvo's standard (`tests/support/marks_measure.rs`): run from
//! `gpui/` with `cargo run -p kuvo_ui --example normalize_marks`. Running it twice changes nothing.
//! `kuvo_ui/tests/marks.rs` holds the files to the result.
#[path = "../tests/support/marks_measure.rs"]
mod marks_measure;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/marks");
    for folder in [root.clone(), root.join("providers")] {
        let mut files: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "svg")).collect();
        files.sort();
        for file in files {
            let svg = std::fs::read_to_string(&file).unwrap();
            let ink = marks_measure::measure(&svg, 480);
            let side = marks_measure::frame_side(&ink);
            let (cx, cy) = (ink.x + ink.w / 2., ink.y + ink.h / 2.);
            let framed = marks_measure::with_frame(&svg, "24", [cx - side / 2., cy - side / 2., side, side]);
            // Measuring is to a pixel, so a file already framed is left as it is.
            let [vx, vy, vw, vh] = ink.view;
            let framed_already = (vw - side).abs() < side * 0.02 && (vh - side).abs() < side * 0.02 && (vx + vw / 2. - cx).abs() < side * 0.02 && (vy + vh / 2. - cy).abs() < side * 0.02;
            if !framed_already {
                std::fs::write(&file, framed).unwrap();
                println!("{}: {:.1}x{:.1} in {:.1} (optical {:.2})", file.file_name().unwrap().to_string_lossy(), ink.w, ink.h, side, marks_measure::optical_side(&ink) / side);
            }
        }
    }
}
