//! Where the desktop app keeps what it keeps as a client: its own folder in `~/.kuvo-code`, on
//! every computer. `~/Library/Application Support/run.kuvo.code` is the earlier Kuvo Code's
//! (0.7's) folder, which the move from it reads and nothing else touches: an entry the app made
//! there would read as 0.7's data on a Mac that never had any.

use std::sync::Arc;

use kuvo::stand_in::{Replies, StandIn};

const NOW: f64 = 1_794_650_000_000.0;

#[test]
fn the_app_keeps_your_computers_in_its_own_folder_and_nothing_in_the_earlier_kuvo_code_s() {
    let home = tempfile::tempdir().unwrap();
    let stand_in = StandIn::at(Replies::Never, NOW);
    // (This device's key: in the Keychain on a Mac, which a test doesn't touch; a file in the
    // app's folder elsewhere, as the app keeps it.)
    let keeper: Arc<dyn kuvo_box::link::Keeper> = if cfg!(target_os = "macos") { Arc::new(kuvo_box::link::MemoryKeeper::default()) } else { Arc::new(kuvo::keeper::PrivateFile::new(kuvo::keeper::folder_of(home.path(), ".kuvo-code").join("identity"))) };
    let boxes = kuvo::keeper::boxes(home.path(), ".kuvo-code", Some(Arc::new(stand_in)), keeper, kuvo_box::link::stand_in::Relay::new().dial());
    // Your computers listed, and this app paired as one of your devices: both kept.
    boxes.read_list().unwrap();
    boxes.settle();
    boxes.pair_again();
    boxes.settle();
    let kept = home.path().join(".kuvo-code/desktop/boxes.json");
    assert!(kept.is_file(), "the list of your computers is kept in ~/.kuvo-code/desktop");
    assert!(std::fs::read_to_string(&kept).unwrap().contains("\"paired\""));
    assert!(!home.path().join("Library").exists(), "nothing is written in ~/Library (the earlier Kuvo Code's folder is run.kuvo.code in its Application Support)");
}
