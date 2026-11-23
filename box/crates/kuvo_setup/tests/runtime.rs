// The setup runtime opens real layout files and publishes its addressed SSH identity.

use std::{fs, path::Path};

use kuvo_server::ssh::{IdentityRefusal, Pairings};
use kuvo_setup::{Error, SetupRuntime};
use kuvo_wire::{
    documents::{self, Envelope, Kind},
    keys::{decode_exact, encode},
};
use serde_json::{Value, json};

fn json_file(home: &Path, name: &str) -> Value {
    serde_json::from_slice(&fs::read(home.join(name)).unwrap()).unwrap()
}

#[tokio::test]
async fn a_machine_recovers_its_signed_identity_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("box");
    let runtime = SetupRuntime::start(&home, "Studio".into(), false, None)
        .await
        .unwrap();
    let link = json_file(&home, "run/link.json");
    let identity = link["setup"]["primary"].as_str().unwrap();
    assert_eq!(
        runtime.handle().identity(identity).unwrap(),
        decode_exact::<32>(link["key"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        runtime.handle().identity("a different computer"),
        Err(IdentityRefusal::WrongComputer)
    );

    let envelope: Envelope = serde_json::from_value(link["leadership"]["1"].clone()).unwrap();
    let cert = documents::verify(envelope).unwrap();
    assert_eq!(cert.kind, Kind::Leadership);
    assert_eq!(encode(&cert.origin), link["setup"]["sign"]);
    assert_eq!(cert.body["setup"]["primary"], identity);
    assert_eq!(cert.body["relayTerm"], 1);

    let card = json_file(&home, "run/setup-card.json");
    assert_eq!(card["formatVersion"], 1);
    assert_eq!(card["computers"], 1);
    assert_eq!(card["primary"]["name"], "Studio");
    drop(runtime);
    let reopened = SetupRuntime::start(&home, "Changed system name".into(), false, None)
        .await
        .unwrap();
    assert!(reopened.handle().identity(identity).is_ok());
    assert_eq!(json_file(&home, "run/link.json"), link);
    assert_eq!(json_file(&home, "run/setup-card.json"), card);
}

#[tokio::test]
async fn a_reserved_account_waits_for_ssh_enrollment() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("box");
    fs::write(root.path().join(".kuvo-code-adding"), []).unwrap();
    let runtime = SetupRuntime::start(&home, "Reserved".into(), false, None)
        .await
        .unwrap();
    assert_eq!(
        runtime.handle().identity("another computer"),
        Err(IdentityRefusal::WrongComputer)
    );
    assert!(!home.join("run/link.json").exists());
    let card = json_file(&home, "run/setup-card.json");
    assert_eq!(card["primary"], Value::Null);
    assert_eq!(card["computers"], 0);
}

#[tokio::test]
async fn an_unconverted_setup_is_refused_before_startup_changes_it() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("box");
    fs::create_dir_all(home.join("run")).unwrap();
    let link = json!({
        "formatVersion": 1,
        "setup": {
            "id": format!("set_{}", encode(&[7; 16])), "epoch": 4, "role": "primary",
            "primary": encode(&[8; 32]), "sign": encode(&[9; 32]), "managed": true,
        }
    });
    fs::write(
        home.join("run/link.json"),
        serde_json::to_vec(&link).unwrap(),
    )
    .unwrap();
    let result = SetupRuntime::start(&home, "Old setup".into(), false, None).await;
    assert!(matches!(result, Err(Error::Version("unconverted setup"))));
    assert_eq!(json_file(&home, "run/link.json"), link);
    assert!(!home.join("run/setup-card.json").exists());
}
