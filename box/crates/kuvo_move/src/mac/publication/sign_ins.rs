//! Interactive handover from the old app's internet passwords to the live vault.
use std::{
    collections::HashSet,
    ffi::OsString,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use kuvo_os::process::{Spawn, Stdio};
use kuvo_store::{format, home::Home, layout};
use reqwest::blocking::{Body, Client};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{App, Outcome, STATE, is_account_home, now, preferences, take_rehome, write_json};
use crate::{Disk, Error, json_at, rehome_status};

const FAILED: &str = "Kuvo couldn't bring them all over this time. Try again.";
const NO_APP: &str = "Kuvo couldn't find its app on this Mac. Open Kuvo, then try again.";
const UNANSWERED: &str = "macOS's question wasn't answered, so nothing more was brought over. Try again when you're at this Mac.";

fn owed(record: &Value) -> bool {
    record["state"] == "done"
        && record["way"] == "move"
        && record["owed"]
            .as_array()
            .is_some_and(|owed| owed.iter().any(|part| part == "passwords"))
}

/// Reject a copied home before looking at the account's live installation.
pub fn owns_sign_ins_home(home: &Home, account: &Path) -> bool {
    home.root()
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(account.canonicalize().ok())
        .is_some_and(|(parent, account)| parent == account)
        && is_account_home(home, account).unwrap_or(false)
}

/// This verb is invoked on purpose. Ordinary startup never calls kuvo-keys
/// for saved passwords, which can each require a macOS authorization prompt.
pub fn sign_ins(home: &Home, account: &Path) -> Result<Outcome, Error> {
    let outcome = |result, message| Outcome {
        result,
        status: rehome_status(home),
        started: None,
        message,
    };
    if !owns_sign_ins_home(home, account) {
        return Ok(outcome("nothing", None));
    }
    let disk = Disk(home.root().into());
    let state = disk.path(STATE)?;
    let format = format::ObjectFormat::additive("Rehome state");
    let read = || -> Result<Value, Error> {
        Ok(format
            .read(&state)?
            .map_or(Value::Null, |object| Value::Object(object.fields)))
    };
    if !owed(&read()?) || super::unfinished(home)?.is_some() {
        return Ok(outcome("nothing", None));
    }
    // Acquire the layout lock only to reclaim an old move lock safely. Opens
    // and updates do not wait for the subsequent Keychain prompts.
    let layout_lock = layout::lock_for_move(home, Duration::from_secs(3))?;
    let move_lock = take_rehome(&disk, Duration::from_secs(3))?;
    drop(layout_lock);
    let mut record = read()?;
    if !owed(&record) {
        return Ok(outcome("nothing", None));
    }
    let mut counts = Counts::default();
    let answer = keys_for(&record, account, home.root())
        .and_then(|(keys, origin)| {
            let prefs = preferences::read(&origin).unwrap_or_default();
            let (ok, reply) = keys.run("passwords", &[], None, Duration::from_secs(1800), 64 << 20)?;
            if !ok || reply.logins.is_none() {
                return Err(match reply.reason.as_deref() {
                    Some("locked") => "Your Mac's Keychain is locked. Unlock it, then try again.",
                    Some("not-allowed") => "macOS didn't let Kuvo read them. Try again: when macOS asks whether kuvo-keys may use each one, enter your Mac's password and click Always Allow.",
                    _ => FAILED,
                });
            }
            let logins = reply.logins.unwrap_or_default();
            if logins.is_empty() {
                return Ok(());
            }
            let vault = Vault::wait(home.root()).ok_or(FAILED)?;
            transfer(&vault, &keys, logins, &Value::Object(prefs), &mut counts)
        });
    let (result, message) = if let Err(message) = answer {
        record["signInsLast"] = json!({"state":"failed","at":now(),"message":message});
        ("failed", Some(message.into()))
    } else {
        let mut notes = record["dropped"].as_array().cloned().unwrap_or_default();
        if counts.short == 1 {
            notes.push(json!("A saved password shorter than four characters wasn't brought into the vault, which keeps none that short."));
        } else if counts.short > 1 {
            notes.push(json!(format!("{} saved passwords shorter than four characters weren't brought into the vault, which keeps none that short.", counts.short)));
        }
        if counts.kept == 1 {
            notes.push(json!("A saved password stays in your Keychain: Kuvo couldn't be sure the vault would keep it exactly. Add any you use to the vault yourself."));
        } else if counts.kept > 1 {
            notes.push(json!(format!("{} saved passwords stay in your Keychain: Kuvo couldn't be sure the vault would keep them exactly. Add any you use to the vault yourself.", counts.kept)));
        }
        record["dropped"] = json!(notes);
        if let Some(owed) = record["owed"].as_array_mut() {
            owed.retain(|part| part != "passwords");
        }
        record["signInsLast"] = json!({"state":"done","at":now(),"added":counts.added,"kept":counts.kept + counts.short});
        ("done", None)
    };
    record["formatVersion"] = json!(1);
    write_json(&state, &record, true)?;
    drop(move_lock);
    Ok(outcome(result, message))
}

fn keys_for(record: &Value, account: &Path, home: &Path) -> Result<(Keys, PathBuf), &'static str> {
    let app: App = serde_json::from_value(record["app"].clone()).map_err(|_| NO_APP)?;
    let origin = account.join("Library/Application Support/run.kuvo.code");
    if app.id != "run.kuvo.code" || record["from"].as_str().map(Path::new) != Some(origin.as_path())
    {
        return Err(NO_APP);
    }
    #[derive(Deserialize)]
    struct Registration {
        desktop: Option<PathBuf>,
    }
    let desktop = json_at::<Registration>(&home.join("run/app.json"))
        .ok()
        .flatten()
        .and_then(|record| record.desktop);
    let executable = [Some(app.bundle), desktop]
        .into_iter()
        .flatten()
        .filter(|bundle| {
            bundle.is_absolute()
                && super::bundle_id(bundle).is_ok_and(|id| {
                    matches!(id.as_str(), "run.kuvo.code" | "run.kuvo.code.desktop")
                })
        })
        .map(|bundle| bundle.join("Contents/MacOS/kuvo-keys"))
        .find(|path| path.is_file())
        .ok_or(NO_APP)?;
    Ok((
        Keys {
            executable,
            account: account.into(),
            home: home.into(),
        },
        origin,
    ))
}

#[derive(Default)]
struct Counts {
    added: usize,
    short: usize,
    kept: usize,
}

#[derive(Deserialize)]
struct Login {
    host: String,
    account: String,
    #[serde(deserialize_with = "secret")]
    password: Zeroizing<String>,
}

fn secret<'de, D: Deserializer<'de>>(reader: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(reader).map(Zeroizing::new)
}

#[derive(Deserialize)]
struct KeyReply {
    reason: Option<String>,
    logins: Option<Vec<Login>>,
}

struct Keys {
    executable: PathBuf,
    account: PathBuf,
    home: PathBuf,
}

impl Keys {
    fn run(
        &self,
        verb: &str,
        args: &[&str],
        input: Option<SecretJson>,
        timeout: Duration,
        max: u64,
    ) -> Result<(bool, KeyReply), &'static str> {
        let mut command = Spawn::new(
            self.executable.clone(),
            [
                ("HOME".into(), self.account.as_os_str().to_owned()),
                ("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()),
            ]
            .into_iter()
            .collect(),
        );
        command.args = [
            OsString::from(verb),
            "--home".into(),
            self.home.as_os_str().to_owned(),
        ]
        .into_iter()
        .chain(args.iter().copied().map(OsString::from))
        .collect();
        command.stdout = Stdio::Pipe;
        if input.is_some() {
            command.stdin = Stdio::Pipe;
        }
        let mut child = command.spawn().map_err(|_| FAILED)?;
        let output = child.stdout.take().ok_or(FAILED)?;
        let reader = thread::spawn(move || {
            let mut bytes = Zeroizing::new(Vec::new());
            if output.take(max + 1).read_to_end(&mut bytes).ok()? as u64 > max {
                return None;
            }
            let line = bytes.trim_ascii().rsplit(|byte| *byte == b'\n').next()?;
            serde_json::from_slice(line).ok()
        });
        let writer = input
            .zip(child.stdin.take())
            .map(|(input, mut pipe)| thread::spawn(move || pipe.write_all(input.as_ref()).is_ok()));
        let began = Instant::now();
        while !child.has_exited().map_err(|_| FAILED)? {
            if began.elapsed() >= timeout {
                let _ = child.stop(Duration::from_secs(1), |_| {});
                return Err(if verb == "passwords" {
                    UNANSWERED
                } else {
                    FAILED
                });
            }
            thread::sleep(Duration::from_millis(25));
        }
        let success = child.wait().map_err(|_| FAILED)?.success();
        if let Some(writer) = writer
            && !writer.join().unwrap_or(false)
        {
            return Err(FAILED);
        }
        let reply = reader.join().ok().flatten().ok_or(FAILED)?;
        Ok((success, reply))
    }
}

#[derive(Serialize)]
struct Password<'a> {
    password: &'a str,
}

struct SecretJson(Zeroizing<Vec<u8>>);

impl SecretJson {
    fn encode(value: impl Serialize) -> Result<Self, &'static str> {
        let mut bytes = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *bytes, &value).map_err(|_| FAILED)?;
        Ok(Self(bytes))
    }
}

impl AsRef<[u8]> for SecretJson {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

#[derive(Deserialize)]
struct Rendezvous {
    port: u16,
    #[serde(deserialize_with = "secret")]
    password: Zeroizing<String>,
}

struct Vault {
    client: Client,
    record: Rendezvous,
}

impl Vault {
    fn wait(home: &Path) -> Option<Self> {
        let file = home.join("run/serve.json");
        let began = Instant::now();
        loop {
            if let Ok(Some(record)) = json_at::<Rendezvous>(&file) {
                let client = Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(60))
                    .build()
                    .ok()?;
                return Some(Self { client, record });
            }
            if began.elapsed() >= Duration::from_secs(30) {
                return None;
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn ask(
        &self,
        method: reqwest::Method,
        route: &str,
        body: Option<SecretJson>,
    ) -> Result<(u16, Value), &'static str> {
        let mut request = self
            .client
            .request(
                method,
                format!("http://127.0.0.1:{}/kuvo/vault{route}", self.record.port),
            )
            .basic_auth("opencode", Some(self.record.password.as_str()))
            .header("x-kuvo-from", "app")
            .header("x-kuvo-internal", "1")
            .header("x-kuvo-route-version", "1");
        if let Some(body) = body {
            let size = body.as_ref().len() as u64;
            request = request
                .header("content-type", "application/json")
                .body(Body::sized(Cursor::new(body), size));
        }
        let reply = request.send().map_err(|_| FAILED)?;
        Ok((reply.status().as_u16(), reply.json().unwrap_or_default()))
    }
}

fn free_name(host: &str, account: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&host.to_lowercase()) {
        return host.into();
    }
    if !account.is_empty() {
        let with_account = format!("{host} ({account})");
        if !taken.contains(&with_account.to_lowercase()) {
            return with_account;
        }
    }
    (2..)
        .map(|index| format!("{host} {index}"))
        .find(|name| !taken.contains(&name.to_lowercase()))
        .expect("finite list of vault item names")
}

fn transfer(
    vault: &Vault,
    keys: &Keys,
    logins: Vec<Login>,
    prefs: &Value,
    tally: &mut Counts,
) -> Result<(), &'static str> {
    use reqwest::Method;
    let (code, listing) = vault.ask(Method::GET, "", None)?;
    if code != 200 {
        return Err(FAILED);
    }
    let mut items = listing["items"].as_array().cloned().ok_or(FAILED)?;
    let mut taken: HashSet<_> = items
        .iter()
        .filter_map(|item| item["name"].as_str())
        .map(str::to_lowercase)
        .collect();
    let mut granted: HashSet<_> = listing["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|grant| grant["who"]["everyone"] == true)
        .filter_map(|grant| grant["item"].as_str().map(str::to_owned))
        .collect();
    let mut seen = HashSet::new();
    let mut duplicates = HashSet::new();
    for login in &logins {
        let pair = (&login.host, &login.account);
        if !seen.insert(pair) {
            duplicates.insert(pair);
        }
    }
    for login in &logins {
        if login.host.is_empty() {
            continue;
        }
        if login.password.contains('\u{fffd}')
            || duplicates.contains(&(&login.host, &login.account))
        {
            tally.kept += 1;
            continue;
        }
        let existing = items.iter().find(|item| {
            item["kind"] == "login"
                && item["username"].as_str().unwrap_or_default() == login.account
                && item["sites"]
                    .as_array()
                    .is_some_and(|sites| sites.iter().any(|site| site == &login.host))
        });
        let item = if let Some(item) = existing {
            item
        } else {
            if login.password.encode_utf16().count() < 4 {
                tally.short += 1;
                continue;
            }
            let name = free_name(&login.host, &login.account, &taken);
            #[derive(Serialize)]
            #[serde(rename_all = "camelCase")]
            struct NewLogin<'a> {
                kind: &'static str,
                name: &'a str,
                sites: [&'a str; 1],
                #[serde(skip_serializing_if = "Option::is_none")]
                username: Option<&'a str>,
                sign_in_only: bool,
                values: Password<'a>,
            }
            let login_body = NewLogin {
                kind: "login",
                name: &name,
                sites: [&login.host],
                username: (!login.account.is_empty()).then_some(&login.account),
                sign_in_only: true,
                values: Password {
                    password: login.password.as_str(),
                },
            };
            let (code, mut made) = vault.ask(
                Method::POST,
                "/items",
                Some(SecretJson::encode(login_body)?),
            )?;
            if code == 400 {
                tally.kept += 1;
                continue;
            }
            if code != 200 {
                return Err(FAILED);
            }
            items.push(made["item"].take());
            taken.insert(name.to_lowercase());
            tally.added += 1;
            items.last().expect("new item")
        };
        let id = item["id"]
            .as_str()
            .filter(|id| {
                id.starts_with("v_") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or(FAILED)?;
        #[derive(Serialize)]
        struct Compare<'a> {
            values: Password<'a>,
        }
        let password = || Password {
            password: login.password.as_str(),
        };
        let (code, result) = vault.ask(
            Method::POST,
            &format!("/items/{id}/same"),
            Some(SecretJson::encode(Compare { values: password() })?),
        )?;
        if code != 200 {
            return Err(FAILED);
        }
        if !result["same"].as_bool().ok_or(FAILED)? {
            tally.kept += 1;
            continue;
        }
        if prefs["passwords.agentSignIn"]
            .as_array()
            .is_some_and(|sites| sites.iter().any(|site| site == &login.host))
            && prefs["passwords.agentSignInAccounts"]
                .get(&login.host)
                .is_some_and(|account| account == &login.account)
            && !granted.contains(id)
        {
            let grant = json!({"item":id,"who":{"everyone":true},"how":"own"});
            if vault
                .ask(Method::POST, "/grants", Some(SecretJson::encode(grant)?))?
                .0
                != 200
            {
                return Err(FAILED);
            }
            granted.insert(id.into());
        }
        let (removed, reply) = keys.run(
            "delete-originals",
            &["--password", &login.host, "--account", &login.account],
            Some(SecretJson::encode(password())?),
            Duration::from_secs(180),
            1 << 20,
        )?;
        if !removed {
            if reply.reason.as_deref() != Some("copy-missing") {
                return Err(FAILED);
            }
            tally.kept += 1;
        }
    }
    Ok(())
}
