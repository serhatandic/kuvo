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
    let runtime = SetupRuntime::prepare(&home, "Studio".into(), false, None)
        .await
        .unwrap()
        .start(None)
        .unwrap();
    let link = json_file(&home, "run/link.json");
    assert_eq!(link["sharedOutcomes"], json!({}));
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
    let reopened = SetupRuntime::prepare(&home, "Changed system name".into(), false, None)
        .await
        .unwrap()
        .start(None)
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
    let runtime = SetupRuntime::prepare(&home, "Reserved".into(), false, None)
        .await
        .unwrap()
        .start(None)
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
    let result = SetupRuntime::prepare(&home, "Old setup".into(), false, None).await;
    assert!(matches!(result, Err(Error::Version("unconverted setup"))));
    assert_eq!(json_file(&home, "run/link.json"), link);
    assert!(!home.join("run/setup-card.json").exists());
}

#[tokio::test]
async fn the_main_saves_exact_device_generations_and_returns_the_original_signed_receipt() {
    use kuvo_server::{Caller, LocalOrigin, Pairing, Transport};
    use kuvo_setup::{ManagedDeviceChange, ManagedDeviceGrant};

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("box");
    let prepared = SetupRuntime::prepare(
        &home,
        "Studio".into(),
        false,
        Some("ws://127.0.0.1:1".into()),
    )
    .await
    .unwrap();
    let runtime = prepared.start(None).unwrap();
    let handle = runtime.handle();
    let key = [13; 32];
    let text = encode(&key);
    let app = Caller::Local(Transport::Http {
        origin: LocalOrigin::App,
        internal: false,
    });
    let direct = Caller::Local(Transport::Http {
        origin: LocalOrigin::Direct,
        internal: false,
    });
    let pair = ManagedDeviceChange::PairDesktop {
        key: text.clone(),
        name: "  Our   desktop ".into(),
    };
    let operation = Some("a".repeat(22));
    assert!(matches!(
        handle
            .change_devices(&direct, operation.clone(), pair.clone())
            .await,
        Err(Error::DevicePermission)
    ));
    let saved = handle
        .change_devices(&app, operation.clone(), pair.clone())
        .await
        .unwrap();
    let pairing = saved.pairing.as_ref().unwrap();
    assert_eq!(pairing.len(), 16);
    assert_eq!(handle.paired(&key).unwrap().name, "Our desktop");
    let receipt = saved.receipt.unwrap();
    let proof = documents::verify(receipt.proof.clone()).unwrap();
    assert_eq!(proof.kind, Kind::SetupShared);
    assert_eq!(proof.body["kind"], "membershipAndAccess");
    assert_eq!(proof.body["operation"], operation.as_deref().unwrap());
    let device = Caller::Paired {
        transport: Transport::AddressedSsh,
        pairing: Pairing::verified(key, pairing.clone(), "Our desktop".into(), true, || true),
    };
    let remove = ManagedDeviceChange::Revoke {
        grants: vec![ManagedDeviceGrant {
            key: text,
            pairing: pairing.clone(),
        }],
    };
    let removed = handle
        .change_devices(&device, Some("b".repeat(22)), remove.clone())
        .await
        .unwrap();
    assert!(removed.receipt.is_some());
    assert!(handle.paired(&key).is_none());

    // The operation receipt identifies the accepted save, even after that device was revoked.
    let replay = handle
        .change_devices(&app, operation, pair.clone())
        .await
        .unwrap();
    assert_eq!(replay.pairing.as_deref(), Some(pairing.as_str()));
    assert_eq!(replay.receipt.unwrap().proof, receipt.proof);
    assert!(handle.paired(&key).is_none());
    assert!(matches!(
        handle.change_devices(&device, None, remove).await,
        Err(Error::DevicePermission)
    ));
    assert!(matches!(
        handle.change_devices(&app, None, pair).await,
        Err(Error::DeviceRemoved)
    ));
    let link = json_file(&home, "run/link.json");
    let membership = &link["shared"]["membershipAndAccess"];
    assert_eq!(membership["sequence"], 3);
    assert_eq!(membership["value"]["devices"]["phones"], json!([]));
    assert_eq!(
        membership["value"]["devices"]["removed"][0]["pairing"],
        pairing.as_str()
    );
}

#[tokio::test]
async fn the_main_reads_and_saves_marketplace_installations_across_restarts() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("box");
    let runtime = SetupRuntime::prepare(&home, "Studio".into(), false, None)
        .await
        .unwrap()
        .start(None)
        .unwrap();
    let handle = runtime.handle();
    let initial = handle.marketplace_setup().unwrap();
    assert!(initial.main_here);
    assert_eq!(initial.sequence, 0);
    assert!(initial.value.is_none());

    let payload = json!({
        "format": 1,
        "apps": [],
        "skills": [],
    });
    let receipt = handle
        .save_marketplace_installations(Some(0), payload.clone())
        .await
        .unwrap();
    let proof = documents::verify(receipt.proof).unwrap();
    assert_eq!(proof.kind, Kind::SetupShared);
    assert_eq!(proof.body["kind"], "marketplaceInstallations");

    let updated = handle.marketplace_setup().unwrap();
    assert_eq!(updated.sequence, 1);
    assert_eq!(updated.value, Some(payload.clone()));
    drop(runtime);

    let reopened = SetupRuntime::prepare(&home, "Studio".into(), false, None)
        .await
        .unwrap()
        .start(None)
        .unwrap();
    let after = reopened.handle().marketplace_setup().unwrap();
    assert_eq!(after.sequence, 1);
    assert_eq!(after.value, Some(payload));
}
