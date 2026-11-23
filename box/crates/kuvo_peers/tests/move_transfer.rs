// TypeScript-compatible bundle parts using the real staging files and move codec.

use kuvo_peers::moves::transfer::{BundleSize, IncomingBundle, OutgoingBundle};
use kuvo_store::files;
use kuvo_wire::keys::{decode, encode};
use sha2::{Digest, Sha256};

#[test]
fn lost_part_acknowledgements_shrink_the_window_and_fresh_ones_reopen_it() {
    use kuvo_peers::moves::transfer::{Flight, PART_BYTES};
    use std::time::Duration;
    use tokio::time::Instant;

    let size = BundleSize::new(PART_BYTES * 32, &encode(&[0; 32])).unwrap();
    let mut flight = Flight::new(&size);
    let started = Instant::now();
    let initial: Vec<_> = flight.due(started).collect();
    assert_eq!(initial, (0..16).collect::<Vec<_>>());
    for index in initial {
        flight.sent(index, started);
    }
    assert_eq!(flight.due(started + Duration::from_secs(19)).count(), 0);
    let retry = started + Duration::from_secs(20);
    let resent: Vec<_> = flight.due(retry).collect();
    assert_eq!(resent, (0..8).collect::<Vec<_>>());
    for index in resent {
        flight.sent(index, retry);
    }
    assert!(flight.acknowledge(0));
    assert!(flight.acknowledge(15)); // The original copy's reply may still arrive.
    assert!(!flight.acknowledge(0));
    assert!(!flight.acknowledge(32));
    assert_eq!(flight.due(retry).collect::<Vec<_>>(), vec![8, 9, 10]);
    assert!(!flight.complete());
    for index in 0..32 {
        flight.acknowledge(index);
    }
    assert!(flight.complete());
    assert_eq!(flight.due(retry).count(), 0);
}

#[test]
fn an_interrupted_copy_recovers_its_parts_and_verifies_the_same_bundle() {
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("source.tgz");
    let destination = folder.path().join("incoming");
    let journal = destination.join("got");
    let bytes: Vec<u8> = (0..60_017).map(|index| (index % 251) as u8).collect();
    let hash = encode(&Sha256::digest(&bytes));
    let size = BundleSize::new(bytes.len() as u64, &hash).unwrap();
    files::write_atomic(&source, &bytes).unwrap();
    let mut sender = OutgoingBundle::open(&source, size.clone()).unwrap();
    let [first, middle, last] = [0, 1, 2].map(|index| sender.part(index).unwrap());
    assert_eq!(decode(&last.encoded).unwrap(), bytes[60_000..]);

    let mut receiver = IncomingBundle::open(&destination, size.clone()).unwrap();
    receiver.receive(&last).unwrap();
    receiver.receive(&last).unwrap();
    assert!(receiver.verify().is_err());
    assert_eq!(files::read(&journal).unwrap().unwrap(), b"2\n");

    // Simulate an incomplete progress write after the process stopped.
    files::write_atomic(&journal, b"2\n0").unwrap();
    let mut receiver = IncomingBundle::open(&destination, size).unwrap();
    receiver.receive(&middle).unwrap();
    assert!(!receiver.complete());
    receiver.receive(&first).unwrap();
    assert_eq!(
        files::read(receiver.verify().unwrap()).unwrap().unwrap(),
        bytes
    );
    assert_eq!(files::read(&journal).unwrap().unwrap(), b"2\n1\n0\n");
}

#[test]
fn acknowledging_all_parts_cannot_prove_a_different_bundle_arrived() {
    let folder = tempfile::tempdir().unwrap();
    let size = BundleSize::new(3, &encode(&Sha256::digest(b"one"))).unwrap();
    let mut receiver = IncomingBundle::open(folder.path(), size).unwrap();
    let part = kuvo_peers::moves::transfer::Part {
        index: 0,
        count: 1,
        encoded: encode(b"two"),
    };
    receiver.receive(&part).unwrap();
    assert!(receiver.complete());
    assert_eq!(
        receiver.verify().unwrap_err().to_string(),
        "What came wasn’t what was sent."
    );
}

#[test]
fn a_manifest_only_bundle_uses_the_move_json_member_in_gzip_tar() {
    use kuvo_peers::moves::{archive, manifest::Manifest};
    use std::{fs::File, io::Read};

    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("bundle.tgz");
    let manifest = Manifest::decode(
        br#"{"kind":"chat","thing":"ses_root","name":"A move","exports":[],"sessionFiles":[]}"#,
    )
    .unwrap();
    archive::write_manifest_only(&path, &manifest).unwrap();
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(&path).unwrap()));
    let mut entries = tar.entries().unwrap();
    let mut entry = entries.next().unwrap().unwrap();
    assert_eq!(
        entry.path().unwrap().as_ref(),
        std::path::Path::new("move.json")
    );
    let mut contents = String::new();
    entry.read_to_string(&mut contents).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&contents).unwrap(),
        serde_json::json!({
            "formatVersion": 1, "kind":"chat", "thing":"ses_root", "name":"A move", "exports":[], "sessionFiles":[],
        })
    );
    assert!(entries.next().is_none());
    assert_eq!(archive::read_manifest(&path).unwrap().thing, "ses_root");
}

#[test]
fn a_link_cannot_supply_the_move_manifest() {
    use kuvo_peers::moves::archive;

    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("bundle.tgz");
    files::write_generated(&path, |file| {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_entry_type(tar::EntryType::Symlink);
        tar.append_link(&mut header, "move.json", "real.json")?;
        tar.into_inner()?.finish()?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        archive::read_manifest(&path),
        Err(archive::Error::Invalid)
    ));
}
