//! Run against the TypeScript-generated scratch home and storage oracle.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kuvo_store::{files, format, home::Home, layout, sqlite};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[derive(Deserialize)]
struct Oracle {
    families: Vec<Family>,
    envelopes: Vec<Envelope>,
}

#[derive(Deserialize)]
struct Family {
    family: String,
    files: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    file: PathBuf,
    hash: String,
}

#[derive(Deserialize)]
struct Envelope {
    kind: Kind,
    file: PathBuf,
    version: Value,
    selection: Option<String>,
    nested: Option<String>,
    #[serde(default)]
    base64: bool,
    expected: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Object,
    List,
    Lines,
    Stamp,
}

#[test]
#[ignore = "requires the scratch oracle from protocol/conformance/compat/storage-golden.mjs"]
fn type_script_beta_storage_matches_the_oracle() {
    let root = PathBuf::from(
        std::env::var_os("STORE_GOLDEN").expect("set STORE_GOLDEN to the oracle's scratch root"),
    );
    let home = Home::new(root.join("machine")).unwrap();
    let oracle: Oracle =
        serde_json::from_slice(&fs::read(root.join("storage-oracle.json")).unwrap()).unwrap();

    layout::open(&home).unwrap();
    assert_eq!(layout::backup_version(&home).unwrap(), 23);
    let mut families = std::collections::BTreeSet::new();
    for family in oracle.families {
        assert!(
            families.insert(family.family.clone()),
            "duplicate family {}",
            family.family
        );
        for sample in family.files {
            let bytes = files::read(&root.join(&sample.file)).unwrap().unwrap();
            assert_eq!(
                hex::encode(Sha256::digest(bytes)),
                sample.hash,
                "{} {}",
                family.family,
                sample.file.display()
            );
        }
    }
    assert_eq!(families.len(), 55);
    assert!(oracle.envelopes.len() >= 38);

    for sample in oracle.envelopes {
        let path = root.join(&sample.file);
        let raw = || fs::read_to_string(&path).unwrap();
        let value = || {
            let mut fields: Value = serde_json::from_str(&raw()).unwrap();
            if let Some(key) = &sample.nested {
                fields = fields[key].take();
            }
            if sample.base64 {
                fields = serde_json::from_slice(
                    &URL_SAFE_NO_PAD.decode(fields.as_str().unwrap()).unwrap(),
                )
                .unwrap();
            }
            fields
        };
        let object = format::ObjectFormat::additive("golden object");
        let fields = match sample.kind {
            Kind::Object if sample.nested.is_none() => {
                Value::Object(object.read(&path).unwrap().unwrap().fields)
            }
            Kind::Object => Value::Object(object.decode(value()).unwrap().fields),
            Kind::List => Value::Array(format::read_additive_list("golden list", value()).unwrap()),
            Kind::Lines => Value::Array(
                raw()
                    .lines()
                    .map(|line| {
                        Value::Object(
                            object
                                .decode(serde_json::from_str(line).unwrap())
                                .unwrap()
                                .fields,
                        )
                    })
                    .collect(),
            ),
            Kind::Stamp => {
                let fields = value();
                let selection = sample.selection.as_deref().unwrap();
                if let Some(version) = sample.version.as_u64() {
                    format::require_stamp("golden stamp", fields.get(selection), version as u32)
                        .unwrap();
                } else {
                    assert_eq!(fields[selection], sample.version);
                }
                fields
            }
        };
        assert_eq!(fields, sample.expected, "{}", sample.file.display());
    }

    let insights = sqlite::open_index(
        &home.root().join("agents/usage/insights.sqlite"),
        sqlite::INSIGHTS_BUSY,
    )
    .unwrap();
    assert_eq!(
        insights
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let fixture = insights
        .query_row("SELECT body FROM state WHERE key = 'fixture'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&fixture).unwrap()["fixture"],
        "golden"
    );
    let search = sqlite::open_index(
        &home
            .root()
            .join("agents/grok/accounts/golden/sessions/session_search.sqlite"),
        sqlite::INSIGHTS_BUSY,
    )
    .unwrap();
    assert_eq!(
        search
            .pragma_query_value(None, "quick_check", |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
