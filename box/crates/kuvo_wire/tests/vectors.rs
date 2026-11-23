//! Shared byte vectors exercised at the wire API.

use kuvo_wire::{frames, keys, noise};
use serde_json::Value;

fn vectors(name: &str) -> Value {
    let source = match name {
        "link.json" => include_str!("../../../../protocol/vectors/link.json"),
        "noise.json" => include_str!("../../../../protocol/vectors/noise.json"),
        "wire.json" => include_str!("../../../../protocol/vectors/wire.json"),
        "docs.json" => include_str!("../../../test/fixtures/docs.json"),
        _ => panic!("unknown vector file {name}"),
    };
    serde_json::from_str(source).expect("the vectors are JSON")
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

fn hex32(value: &Value) -> [u8; 32] {
    hex(value.as_str().expect("hex text"))
        .try_into()
        .expect("32 bytes")
}

// MARK: Vectors

#[test]
fn the_handshake_and_transport_match_the_noise_vectors() {
    let all = vectors("noise.json");
    let vector = all["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["protocol_name"] == "Noise_IK_25519_AESGCM_SHA256")
        .expect("the IK vector");
    let prologue = hex(vector["init_prologue"].as_str().unwrap());
    let messages = vector["messages"].as_array().unwrap();
    let mut initiator = noise::Initiator::with_ephemeral(
        hex32(&vector["init_static"]),
        hex32(&vector["init_remote_static"]),
        &prologue,
        hex32(&vector["init_ephemeral"]),
    );
    let mut responder = noise::Responder::with_ephemeral(
        hex32(&vector["resp_static"]),
        &prologue,
        hex32(&vector["resp_ephemeral"]),
    );
    let payload = |index: usize| hex(messages[index]["payload"].as_str().unwrap());
    let ciphertext = |index: usize| hex(messages[index]["ciphertext"].as_str().unwrap());

    let first = initiator.write_message_1(&payload(0)).unwrap();
    assert_eq!(first, ciphertext(0));
    assert_eq!(responder.read_message_1(&first).unwrap(), payload(0));
    let (second, mut answering) = responder.write_message_2(&payload(1)).unwrap();
    assert_eq!(second, ciphertext(1));
    let (read, mut asking) = initiator.read_message_2(&second).unwrap();
    assert_eq!(read, payload(1));
    assert_eq!(
        asking.hash.to_vec(),
        hex(vector["handshake_hash"].as_str().unwrap())
    );
    for index in 2..messages.len() {
        let (from, to) = if index % 2 == 0 {
            (&mut asking, &mut answering)
        } else {
            (&mut answering, &mut asking)
        };
        let sealed = from.seal(&payload(index));
        assert_eq!(sealed, ciphertext(index), "message {index}");
        assert_eq!(to.open(&sealed).unwrap(), payload(index));
    }
    // Anything that doesn't open ends the session: it's an error, never bytes.
    assert!(asking.open(b"not a sealed message").is_err());
}

#[test]
fn addresses_frames_and_messages_match_the_link_vectors() {
    let link = vectors("link.json");
    for case in link["addresses"].as_array().unwrap() {
        assert_eq!(
            keys::address_of(&hex32(&case["secret"])).to_vec(),
            hex(case["address"].as_str().unwrap())
        );
    }
    for case in link["relay"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let name = case["name"].as_str().unwrap();
        let address = || -> [u8; 16] { hex(case["address"].as_str().unwrap()).try_into().unwrap() };
        match (case["from"].as_str().unwrap(), name) {
            ("device", "hello") => {
                assert_eq!(frames::relay_hello(&bytes[1..].try_into().unwrap()), bytes)
            }
            ("device", "send") => assert_eq!(
                frames::relay_send(
                    &hex(case["to"].as_str().unwrap()).try_into().unwrap(),
                    &hex(case["payload"].as_str().unwrap())
                ),
                bytes
            ),
            ("relay", "welcome") => assert_eq!(
                frames::read_relay(&bytes),
                Some(frames::FromRelay::Welcome(address()))
            ),
            ("relay", "deliver") => assert_eq!(
                frames::read_relay(&bytes),
                Some(frames::FromRelay::Deliver {
                    from: address(),
                    payload: hex(case["payload"].as_str().unwrap())
                })
            ),
            ("relay", "unreachable") => assert_eq!(
                frames::read_relay(&bytes),
                Some(frames::FromRelay::Unreachable(address()))
            ),
            ("relay", "pushed") => assert_eq!(
                frames::read_relay(&bytes),
                Some(frames::FromRelay::Pushed {
                    to: address(),
                    status: case["status"].as_u64().unwrap() as u8
                })
            ),
            ("relay", "gone") => assert_eq!(
                frames::read_relay(&bytes),
                Some(frames::FromRelay::Gone {
                    address: address(),
                    replaced: case["reads"] == "replaced"
                })
            ),
            ("device", "push") => assert_eq!(
                frames::relay_push(&bytes[1..17].try_into().unwrap(), &bytes[17..]),
                bytes
            ),
            ("device", "register") => {} // Retired APNs registration.
            _ => panic!("unexpected relay vector {name}"),
        }
    }
    for case in link["frames"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let (kind, session, body) = frames::read_frame(&bytes).expect("a frame");
        assert_eq!(
            (kind as u64, session as u64, body.to_vec()),
            (
                case["kind"].as_u64().unwrap(),
                case["session"].as_u64().unwrap(),
                hex(case["body"].as_str().unwrap())
            )
        );
        assert_eq!(frames::frame(kind, session, body), bytes);
    }
    for case in link["messages"].as_array().unwrap() {
        let bytes = hex(case["bytes"].as_str().unwrap());
        let body = hex(case["body"].as_str().unwrap());
        assert_eq!(
            frames::encode_message(&case["head"], &body),
            bytes,
            "{}",
            case["head"]
        );
        let (head, read) = frames::decode_message(&bytes).expect("a message");
        assert_eq!((head, read.to_vec()), (case["head"].clone(), body));
    }
}

#[test]
fn one_way_noise_matches_the_handshake_and_following_messages() {
    let all = vectors("noise.json");
    let vector = &all["vectors"][1];
    assert_eq!(vector["protocol_name"], "Noise_K_25519_AESGCM_SHA256");
    let prologue = hex(vector["init_prologue"].as_str().unwrap());
    let messages = vector["messages"].as_array().unwrap();
    let payload = |message: &Value| hex(message["payload"].as_str().unwrap());
    let ciphertext = |message: &Value| hex(message["ciphertext"].as_str().unwrap());
    let (message, mut sender) = noise::seal_one_way_with_ephemeral(
        &payload(&messages[0]),
        &hex32(&vector["init_static"]),
        &hex32(&vector["init_remote_static"]),
        &prologue,
        &hex32(&vector["init_ephemeral"]),
    )
    .unwrap();
    assert_eq!(message, ciphertext(&messages[0]));
    let (plain, mut receiver) = noise::open_one_way(
        &message,
        &hex32(&vector["resp_static"]),
        &hex32(&vector["resp_remote_static"]),
        &prologue,
    )
    .unwrap();
    assert_eq!(plain, payload(&messages[0]));
    assert_eq!(
        receiver.hash.as_slice(),
        hex(vector["handshake_hash"].as_str().unwrap())
    );
    assert_eq!(sender.hash, receiver.hash);
    for each in &messages[1..] {
        let sealed = sender.seal(&payload(each));
        assert_eq!(sealed, ciphertext(each));
        assert_eq!(receiver.open(&sealed).unwrap(), payload(each));
    }
}

#[test]
fn the_box_refuses_malformed_and_low_order_handshakes() {
    let mut initiator =
        noise::Initiator::with_ephemeral([1; 32], [0; 32], noise::PROLOGUE, [2; 32]);
    assert!(initiator.write_message_1(b"hello").is_err());
    let mut responder = noise::Responder::with_ephemeral([1; 32], noise::PROLOGUE, [2; 32]);
    assert!(responder.read_message_1(&[0; 96]).is_err());
    assert!(noise::open_one_way(&[0; 48], &[1; 32], &[2; 32], noise::MAIL_PROLOGUE).is_err());
}

#[test]
fn all_peer_mail_envelopes_match_the_typescript_vectors() {
    use kuvo_wire::mail::Envelope;
    let all = vectors("wire.json");
    let from = hex32(&all["from"]);
    let to = hex32(&all["to"]);
    let ephemeral = hex32(&all["ephemeral"]);
    let from_public = hex32(&all["fromPublic"]);
    let to_public = hex32(&all["toPublic"]);
    for case in all["mail"].as_array().unwrap() {
        let message = Envelope(case["payload"].as_object().unwrap().clone());
        let removal = case["removal"].as_bool().unwrap();
        assert_eq!(
            message.encode(),
            case["plaintext"].as_str().unwrap().as_bytes(),
            "{}",
            case["name"]
        );
        let frame = message
            .seal_with_ephemeral(&from, &to_public, &ephemeral, removal)
            .unwrap();
        assert_eq!(
            frame,
            hex(case["frame"].as_str().unwrap()),
            "{}",
            case["name"]
        );
        let (kind, number, body) = frames::read_frame(&frame).unwrap();
        assert_eq!((kind, number), (frames::Kind::Mail, 0));
        if removal {
            assert_eq!(
                Envelope::open_removal(body, &to),
                Some((from_public, message))
            );
        } else {
            assert_eq!(Envelope::open(body, &to, &from_public).unwrap(), message);
            assert!(Envelope::open(body, &to, &to_public).is_err());
        }
    }
}

#[test]
fn legacy_mail_is_read_and_future_mail_is_refused() {
    use kuvo_wire::mail::{Envelope, Error};
    let legacy = Envelope::decode(br#"{"at":1,"agents":{}}"#).unwrap();
    assert_eq!(
        legacy.encode(),
        br#"{"at":1,"agents":{},"formatVersion":1}"#
    );
    assert_eq!(
        Envelope::decode(br#"{"formatVersion":2}"#),
        Err(Error::Version)
    );
    assert_eq!(
        Envelope::decode(br#"{"formatVersion":null}"#),
        Err(Error::Version)
    );
    assert_eq!(Envelope::decode(br#"[]"#), Err(Error::Shape));
}

#[test]
fn signed_documents_verify_or_fail_as_the_shared_cases_require() {
    use kuvo_wire::documents::{Envelope, Parts, split, verify};
    let docs = vectors("docs.json");
    for case in docs["taken"].as_array().unwrap() {
        let envelope: Envelope = serde_json::from_value(case["envelope"].clone()).unwrap();
        let document = verify(envelope.clone()).unwrap();
        assert_eq!(document.envelope, envelope);
        assert_eq!(keys::encode(&document.origin), case["origin"]);
        assert_eq!(serde_json::to_value(document.kind).unwrap(), case["kind"]);
        assert_eq!(document.n, case["n"].as_u64().unwrap());
        assert_eq!(document.digest(), case["h"]);
    }
    for case in docs["refused"].as_array().unwrap() {
        let envelope = serde_json::from_value(case["envelope"].clone()).unwrap();
        assert!(verify(envelope).is_err(), "{}", case["name"]);
    }
    let big = &docs["parts"];
    let parts: Vec<kuvo_wire::documents::Part> =
        serde_json::from_value(big["parts"].clone()).unwrap();
    let full = Envelope {
        d: parts.iter().map(|part| part.d.as_str()).collect(),
        s: parts[0].s.clone(),
    };
    assert_eq!(split(&full).unwrap(), parts);
    let mut pending = Parts::default();
    for part in parts.iter().rev().skip(1) {
        assert!(pending.receive([1; 32], part.clone()).unwrap().is_none());
    }
    let last = parts.last().unwrap();
    assert!(
        pending.receive([2; 32], last.clone()).unwrap().is_none(),
        "another sender cannot complete it"
    );
    let verified = pending.receive([1; 32], last.clone()).unwrap().unwrap();
    assert_eq!(verified.n, big["n"].as_u64().unwrap());
    assert_eq!(keys::encode(&verified.origin), big["origin"]);
    assert_eq!(verified.envelope, full);
    let mut changed = last.clone();
    changed.d.push('x');
    for part in &parts[..parts.len() - 1] {
        assert!(pending.receive([1; 32], part.clone()).unwrap().is_none());
    }
    assert!(pending.receive([1; 32], changed).is_err());
}

#[test]
fn generated_signatures_and_canonical_digests_match_typescript_bytes() {
    use kuvo_wire::{documents, json};
    let all = vectors("wire.json");
    for case in all["documents"].as_array().unwrap() {
        let body = case["body"].as_object().unwrap().clone();
        let kind = serde_json::from_value(case["kind"].clone()).unwrap();
        let envelope = documents::sign(
            &hex32(&all["seed"]),
            kind,
            case["n"].as_u64().unwrap(),
            body,
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&envelope).unwrap(), case["envelope"]);
        assert_eq!(
            documents::verify(envelope).unwrap().digest(),
            case["digest"]
        );
    }
    for case in all["digests"].as_array().unwrap() {
        assert_eq!(json::canonical(&case["value"]), case["canonical"]);
        assert_eq!(keys::encode(&json::digest(&case["value"])), case["sha256"]);
    }
}
