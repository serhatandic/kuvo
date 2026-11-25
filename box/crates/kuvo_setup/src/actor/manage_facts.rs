//! Who keeps the machines' records, as the computers list needs it to offer Rename and Remove.

use kuvo_peers::{
    devices::{RecordWriter, record_writer, released_macs},
    manage::{Candidate, ManageFacts, SetupFacts, Writer},
};

use super::*;

pub(super) fn manage_facts(
    files: &Files,
    link: &Link,
    own: Option<&Identity>,
    kind: ComputerKind,
) -> ManageFacts {
    let own = own.map(|own| own.public.as_str());
    let setup = link.setup().ok().filter(|setup| !setup.aside);
    let membership = setup.and_then(|_| link.member_list().ok());
    let named = |key: &str, fallback: &str| {
        link.value(SharedKind::SetupNames)
            .and_then(|names| names.get(key))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    };
    let main_here = setup.is_some_and(|setup| {
        setup.role == Role::Primary && Some(setup.position.primary.as_str()) == own
    });
    let mut machines = std::collections::BTreeSet::new();
    let writer = match setup {
        Some(setup) if setup.role == Role::Member => {
            let primary = &setup.position.primary;
            let listed = membership
                .as_ref()
                .and_then(|members| members.boxes.iter().find(|peer| &peer.key == primary))
                .map_or("", |peer| peer.name.as_str());
            let name = named(primary, listed);
            Writer::Elsewhere {
                key: primary.clone(),
                name: if name.is_empty() { "your main computer".into() } else { name },
            }
        }
        _ if kind == ComputerKind::Mac || main_here => Writer::Here,
        _ => match record_writer(&link.extra, link.key.as_deref()) {
            RecordWriter::Mac(mac) if setup.is_none() => {
                machines.extend(mac.machines.iter().map(|key| kuvo_wire::keys::encode(key)));
                Writer::Elsewhere {
                    key: kuvo_wire::keys::encode(&mac.key),
                    name: mac.name,
                }
            }
            _ => Writer::None,
        },
    };
    let managed = setup.is_some_and(|setup| setup.managed);
    if let Some(members) = &membership {
        machines = members
            .boxes
            .iter()
            .filter(|peer| managed || peer.kind == ComputerKind::Machine)
            .map(|peer| peer.key.clone())
            .collect();
    }
    if let Some(setup) = setup.filter(|setup| setup.managed) {
        machines.insert(setup.position.primary.clone());
    }
    ManageFacts {
        setup: setup.map(|setup| SetupFacts {
            primary: setup.position.primary.clone(),
            main_here,
            managed,
            checkpointed: setup.checkpointed,
            boxes: membership
                .iter()
                .flat_map(|members| &members.boxes)
                .map(|peer| Candidate {
                    key: peer.key.clone(),
                    name: named(&peer.key, &peer.name),
                    machine: peer.kind == ComputerKind::Machine,
                    signed: peer.sign.is_some(),
                    provisional: replication::provisional_in(link, &peer.key),
                })
                .collect(),
        }),
        writer,
        macs: if setup.is_some() { 0 } else { released_macs(&link.extra) },
        machines,
        wipe_here: crate::wipe::supported_here(files.root()),
    }
}
