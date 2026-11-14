//! Accounts and signing in (PROTOCOL.md, "Signing in" and the coding agents' accounts): the
//! engine's plans and providers (`api/integration`) and the coding agents' accounts
//! (`kuvo/agents/api/integration`), with which of them Accounts lists, each one's mark, the ways in,
//! their order and their words as the box says them (`kuvo/sign-in/accounts`,
//! `kuvo/sign-in/methods`), how each account stands, the line under its name and what can be done
//! with it, each with its button's and its confirm's words, as the box says them too
//! (`sign-in.accounts.state`), a sign-in in the browser started through the box with its device code
//! apart (`kuvo/sign-in/start`), how it goes (`kuvo/sign-in/status`, with what to say when it didn't
//! work), and signing out through the engine's and the bridge's own routes. The engine does the
//! signing in; nothing here reimplements it, decides a way in, what's listed, how an account stands
//! or what it offers, reads a code out of a page, or words a failure.
//!
//! Never read here: a key or a sign-in's secret (`api/credential` lists them; this never asks it).

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::BoxClient;
use crate::error::{Error, Refusal, Result};
use crate::paths::{query_value, segment};
use crate::transport::Request;

/// Which side of the box an account lives on: the engine's plans and providers, or the coding
/// agents' bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Engine,
    Agents,
}

impl Side {
    /// Where its `api/…` routes are.
    fn api(self) -> &'static str {
        match self {
            Side::Engine => "api",
            Side::Agents => "kuvo/agents/api",
        }
    }

    fn agents_query(self) -> &'static str {
        match self {
            Side::Engine => "",
            Side::Agents => "&agents=1",
        }
    }
}

/// A field a way in asks for besides its key (Amazon Bedrock's region), as the engine describes it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FormField {
    pub key: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub required: bool,
    /// `text`, `string`, `boolean`, …: this app draws text and on/off; any other kind is said to
    /// need Kuvo Code on the Mac or iPhone.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub placeholder: Option<String>,
}

impl FormField {
    /// Its name, as the engine gives it.
    pub fn name(&self) -> String {
        self.title.clone().or_else(|| self.label.clone()).unwrap_or_else(|| self.key.clone())
    }

    /// Whether this app can draw it: a text field or an on/off switch.
    pub fn drawable(&self) -> bool {
        matches!(self.kind.as_deref(), None | Some("text" | "string" | "boolean"))
    }

    pub fn is_switch(&self) -> bool {
        self.kind.as_deref() == Some("boolean")
    }
}

/// One way into an account, as the box offers it: its kind (`oauth`, `key`, `external`), the
/// engine's id for it, the button's words, what else it asks for, and whether it's offered apart.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Method {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub form: Vec<FormField>,
    #[serde(default)]
    pub legacy: bool,
}

/// An account's ways in, in the box's order, with the environment variables that sign in too and
/// the line its card names under its name.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Ways {
    #[serde(default, deserialize_with = "crate::accounts::readable")]
    pub methods: Vec<Method>,
    #[serde(default)]
    pub environment: Vec<String>,
    #[serde(default)]
    pub hint: String,
}

impl Ways {
    /// The ways offered first, in order.
    pub fn primary(&self) -> impl Iterator<Item = &Method> {
        self.methods.iter().filter(|method| !method.legacy)
    }

    /// Those offered apart, after them ("Other ways to sign in").
    pub fn legacy(&self) -> impl Iterator<Item = &Method> {
        self.methods.iter().filter(|method| method.legacy)
    }
}

/// One account the engine or the coding agents' bridge lists (`…/api/integration`): its id and
/// its name. How it's signed in is the box's to say (`state`, `line`), never read here.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Integration {
    pub id: String,
    pub name: String,
}

/// How an account stands, as the box says (`state`): the fact its line words. One this client
/// doesn't know reads as not signed in (PROTOCOL.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    SignedIn,
    /// Its sign-in ended: signed in again, it works.
    Ended,
    /// A coding agent's program isn't on the computer.
    NotInstalled,
    /// The agent couldn't say just now.
    Unchecked,
    /// The sign-in in use says something's wrong, in its own words (the line).
    Problem,
    #[default]
    #[serde(other)]
    SignedOut,
}

impl State {
    /// Whether it needs you: drawn as a warning.
    pub fn needs_you(self) -> bool {
        matches!(self, State::Ended | State::Problem)
    }
}

/// What a client asks before it does an action, in the box's words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Confirm {
    pub title: String,
    pub message: String,
    /// The button that goes ahead.
    pub label: String,
}

/// Something that can be done with an account now, as the box says: which, its button's words,
/// its confirm where it asks first, and whether it only works on the box's computer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Action {
    /// `signIn`, `signInAgain`, `addAnother`, `disconnect`, `install`, `update`, `signOut`,
    /// `rename`, `remove`.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub confirm: Option<Confirm>,
    #[serde(default)]
    pub here: bool,
}

impl Action {
    /// Whether a client offers it: one that only works on the box's computer, only where the
    /// client runs there (`here`).
    pub fn offered(&self, here: bool) -> bool {
        here || !self.here
    }

    /// Whether it starts a sign-in from the account's ways in.
    pub fn signs_in(&self) -> bool {
        matches!(self.id.as_str(), "signIn" | "signInAgain" | "addAnother")
    }
}

/// An account as Accounts draws it: the engine's or the bridge's listing, and what the box says
/// of it (`kuvo/sign-in/accounts`): its ways in, its mark, how it stands, its line and its actions.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountEntry {
    pub side: Side,
    pub integration: Integration,
    /// Its mark, by the plan limits' service ids (`claude`, `copilot`…), as the box says; none for
    /// one with no mark, drawn by its name's first letter.
    pub service: Option<String>,
    /// The box's ways in, for the sign-in sheet.
    pub ways: Ways,
    pub state: State,
    /// The line under its name.
    pub line: String,
    /// What can be done with it now, in the box's order.
    pub actions: Vec<Action>,
}

/// Every account on this box, as Accounts lists them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Accounts {
    /// The coding agents' accounts, in the bridge's order. Empty where its bridge isn't running
    /// (`agents_down` says why).
    pub agents: Vec<AccountEntry>,
    /// The engine's plans and providers, in its order: those the box says Accounts lists.
    pub engine: Vec<AccountEntry>,
    /// Why the coding agents' bridge, or the engine, couldn't list its accounts just now (`503`).
    pub agents_down: Option<String>,
    pub engine_down: Option<String>,
}

/// A sign-in under way, as the engine or the bridge started it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Attempt {
    #[serde(rename = "attemptID")]
    pub id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub instructions: String,
    /// `auto`: it comes back by itself; `code`: you paste a code back.
    #[serde(default)]
    pub mode: String,
    /// It may end on a code to paste back instead (Claude Code's).
    #[serde(default, rename = "acceptsCode")]
    pub accepts_code: bool,
    /// The code its page asks for, as the box says it apart.
    #[serde(default, rename = "userCode")]
    pub user_code: Option<String>,
}

/// How a sign-in goes, as the box says (`kuvo/sign-in/status`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Status {
    /// `pending`, `complete`, `failed`, `expired`.
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default, rename = "userCode")]
    pub user_code: Option<String>,
    /// What to show once it `failed` or `expired`, in the box's words.
    #[serde(default)]
    pub failure: Option<String>,
}

impl Status {
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    pub fn is_complete(&self) -> bool {
        self.status == "complete"
    }

    /// What went wrong, in the box's words (its engine's where the box gave none).
    pub fn failure_words(&self) -> Option<String> {
        matches!(self.status.as_str(), "failed" | "expired").then(|| self.failure.clone().or_else(|| self.message.clone()).unwrap_or_default())
    }
}

/// What installing a coding agent's program came to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Installed {
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub version: Option<String>,
    /// Why not, in the installer's words.
    #[serde(default)]
    pub message: Option<String>,
}

/// A list's items this can read: one it can't is left out, not all of them.
pub(crate) fn readable<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(deserializer: D) -> std::result::Result<Vec<T>, D::Error> {
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect())
}

/// The data an engine or bridge answer carries (`{data: …}`).
#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

impl BoxClient {
    /// One side's accounts, with the box's ways in.
    fn side_accounts(&self, side: Side) -> Result<Vec<AccountEntry>> {
        let listed_here: Data<Vec<Value>> = self.get(&format!("{}/integration", side.api()))?;
        let integrations: Vec<Integration> = listed_here.data.into_iter().filter_map(|value| serde_json::from_value(value).ok()).collect();
        #[derive(Deserialize)]
        struct Entry {
            id: String,
            #[serde(flatten)]
            ways: Ways,
            /// Whether Accounts lists it, as the box decides (whatever `kind` it names).
            #[serde(default)]
            listed: bool,
            #[serde(default)]
            service: Option<String>,
            #[serde(default)]
            state: State,
            #[serde(default)]
            line: String,
            #[serde(default, deserialize_with = "readable")]
            actions: Vec<Action>,
        }
        #[derive(Deserialize)]
        struct Answer {
            #[serde(default, deserialize_with = "readable")]
            accounts: Vec<Entry>,
        }
        let route = match side {
            Side::Engine => "kuvo/sign-in/accounts",
            Side::Agents => "kuvo/sign-in/accounts?agents=1",
        };
        let ways: Vec<Entry> = self.get::<Answer>(route).map(|answer| answer.accounts)?;
        // Those the box says Accounts lists (`listed`), each as the box says it.
        Ok(integrations
            .into_iter()
            .filter_map(|integration| {
                let entry = ways.iter().find(|entry| entry.id == integration.id).filter(|entry| entry.listed)?;
                Some(AccountEntry { side, service: entry.service.clone(), ways: entry.ways.clone(), state: entry.state, line: entry.line.clone(), actions: entry.actions.clone(), integration })
            })
            .collect())
    }

    /// Every account on this box: the coding agents' (where their bridge runs) and the engine's.
    pub fn sign_in_accounts(&self) -> Result<Accounts> {
        // Either side not running (503) leaves its accounts out, saying why; the other's stay.
        let side = |side: Side| match self.side_accounts(side) {
            Ok(accounts) => Ok((accounts, None)),
            Err(Error::Refused(refusal)) if refusal.status == 503 => Ok((Vec::new(), Some(refusal.message))),
            Err(error) => Err(error),
        };
        let (engine, engine_down) = side(Side::Engine)?;
        let (agents, agents_down) = side(Side::Agents)?;
        Ok(Accounts { agents, engine, agents_down, engine_down })
    }

    /// An account's ways in, as the box offers them now.
    pub fn sign_in_methods(&self, side: Side, account: &str) -> Result<Ways> {
        self.get(&format!("kuvo/sign-in/methods?integration={}{}", query_value(account), side.agents_query()))
    }

    /// Starts a sign-in in the browser (`oauth`), with what its form asked, through the box: its
    /// answer is the engine's (or the bridge's) attempt, with the code its page asks for apart
    /// (`userCode`) where it has one.
    pub fn start_sign_in(&self, side: Side, account: &str, method: &str, answer: Option<&Map<String, Value>>) -> Result<Attempt> {
        let mut body = json!({ "methodID": method });
        if let Some(answer) = answer.filter(|answer| !answer.is_empty()) {
            body["answer"] = Value::Object(answer.clone());
        }
        self.value(&Request::post(format!("kuvo/sign-in/start?integration={}{}", query_value(account), side.agents_query()), body))
    }

    /// How a sign-in goes, as the box says (the engine's status, with its failure in words).
    pub fn sign_in_status(&self, side: Side, account: &str, attempt: &str) -> Result<Status> {
        let wait = if self.streams() { "" } else { "&wait=20" };
        self.get(&format!("kuvo/sign-in/status?integration={}&attempt={}{}{wait}", query_value(account), query_value(attempt), side.agents_query()))
    }

    /// Hands the engine the code a sign-in's page gave.
    pub fn complete_sign_in(&self, side: Side, account: &str, attempt: &str, code: &str) -> Result<()> {
        self.done(&Request::post(format!("{}/integration/{}/connect/oauth/{}/complete", side.api(), segment(account), segment(attempt)), json!({ "code": code })))
    }

    /// Gives a sign-in up.
    pub fn cancel_sign_in(&self, side: Side, account: &str, attempt: &str) -> Result<()> {
        self.done(&Request::delete(format!("{}/integration/{}/connect/oauth/{}", side.api(), segment(account), segment(attempt))))
    }

    /// Signs in with a key, and what its form asked. The key goes to the engine and nowhere else.
    pub fn add_key(&self, side: Side, account: &str, key: &str, answer: Option<&Map<String, Value>>) -> Result<()> {
        let mut body = json!({ "key": key });
        if let Some(answer) = answer.filter(|answer| !answer.is_empty()) {
            body["answer"] = Value::Object(answer.clone());
        }
        self.done(&Request::post(format!("{}/integration/{}/connect/key", side.api(), segment(account)), body))
    }

    /// Signs in with credentials the computer has already (AWS's, gcloud's).
    pub fn use_external(&self, side: Side, account: &str, method: &str, answer: Option<&Map<String, Value>>) -> Result<()> {
        let mut body = json!({ "methodID": method });
        if let Some(answer) = answer.filter(|answer| !answer.is_empty()) {
            body["answer"] = Value::Object(answer.clone());
        }
        self.done(&Request::post(format!("{}/integration/{}/connect/external", side.api(), segment(account)), body))
    }

    /// Installs a coding agent's program on this computer, with its maker's installer.
    pub fn install_agent(&self, account: &str) -> Result<Installed> {
        let data: Data<Installed> = self.value(&Request::post(format!("kuvo/agents/api/integration/{}/install", segment(account)), json!({})))?;
        Ok(data.data)
    }

    /// Disconnect: this box's sign-ins and keys for one of the engine's plans or providers.
    pub fn forget_account(&self, account: &str) -> Result<()> {
        self.done(&Request::post(format!("kuvo/accounts/{}/forget", segment(account)), json!({})))
    }

    /// Signs a coding agent out, where it says it can be (`metadata.signsOut`).
    pub fn sign_out_agent(&self, account: &str) -> Result<()> {
        self.done(&Request::post(format!("kuvo/agents/api/integration/{}/disconnect", segment(account)), json!({})))
    }

    /// Removes an account you added beside an agent's own sign-in.
    pub fn remove_agent_account(&self, account: &str) -> Result<()> {
        self.done(&Request::delete(format!("kuvo/agents/api/kuvo/accounts/{}", segment(account))))
    }

    /// Sends `request`; anything but a success is the box's refusal.
    pub(crate) fn done(&self, request: &Request) -> Result<()> {
        let response = self.send(request)?;
        if response.is_success() { Ok(()) } else { Err(Error::Refused(Refusal::from_answer(response.status, &response.body))) }
    }
}

/// A refusal's words, for a card or a line under a field.
pub fn said(error: &Error) -> String {
    match error {
        Error::Refused(refusal) => refusal.message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The box's ways in, read as `Vectors/sign-in.json` records them: each kind, its words and
    /// form, the legacy ones apart, and an account with none.
    #[test]
    fn ways_in_read_as_the_shared_vectors_say_them() {
        let vectors: Value = serde_json::from_str(include_str!("../../../protocol/vectors/sign-in.json")).unwrap();
        for case in vectors["methods"].as_array().unwrap() {
            let offered = &case["offered"];
            let ways: Ways = serde_json::from_value(offered.clone()).unwrap();
            assert_eq!(ways.methods.len(), offered["methods"].as_array().unwrap().len(), "{}", case["provider"]);
            assert_eq!(ways.hint, offered["hint"].as_str().unwrap());
            let legacy = offered["methods"].as_array().unwrap().iter().filter(|method| method["legacy"] == json!(true)).count();
            assert_eq!(ways.legacy().count(), legacy);
            assert_eq!(ways.primary().count() + legacy, ways.methods.len());
        }
    }

    /// A sign-in the box started reads with its device code apart, as `Vectors/sign-in.json`'s
    /// `codes` record it: shown as the box says it, never read out of the page or instructions here.
    #[test]
    fn a_started_sign_ins_code_is_the_one_the_box_says() {
        let vectors: Value = serde_json::from_str(include_str!("../../../protocol/vectors/sign-in.json")).unwrap();
        let cases = vectors["codes"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for case in cases {
            // The box's answer: the attempt, with `userCode` where it found one.
            let mut started = case["attempt"].clone();
            started["attemptID"] = json!("att_1");
            match case.get("userCode") {
                Some(code) => started["userCode"] = code.clone(),
                None => drop(started.as_object_mut().unwrap().remove("userCode")),
            }
            let attempt: Attempt = serde_json::from_value(started).unwrap();
            assert_eq!(attempt.user_code.as_deref(), case.get("userCode").and_then(Value::as_str), "{}", case["about"]);
        }
    }

    /// How an account stands and what can be done with it, read as `Vectors/sign-in.json`'s
    /// `states` record the box saying them: every state and action there has a name here, and a
    /// confirm's words come whole.
    #[test]
    fn an_accounts_state_and_actions_read_as_the_shared_vectors_say_them() {
        let vectors: Value = serde_json::from_str(include_str!("../../../protocol/vectors/sign-in.json")).unwrap();
        let cases = vectors["states"].as_array().unwrap();
        assert!(cases.len() >= 41);
        for case in cases {
            let state: State = serde_json::from_value(case["state"].clone()).unwrap();
            assert_eq!(state == State::SignedOut, case["state"] == "signedOut", "{}", case["about"]);
            let actions: Vec<Action> = serde_json::from_value(case["actions"].clone()).unwrap();
            for (action, said) in actions.iter().zip(case["actions"].as_array().unwrap()) {
                assert_eq!(action.label, said["label"].as_str().unwrap());
                assert_eq!(action.here, said["here"] == json!(true), "{}", case["about"]);
                assert_eq!(action.confirm.as_ref().map(|confirm| (confirm.title.as_str(), confirm.message.as_str(), confirm.label.as_str())), said.get("confirm").map(|confirm| (confirm["title"].as_str().unwrap(), confirm["message"].as_str().unwrap(), confirm["label"].as_str().unwrap())));
            }
        }
        // A state a newer box names reads as not signed in.
        assert_eq!(serde_json::from_value::<State>(json!("somethingNew")).unwrap(), State::SignedOut);
    }

    #[test]
    fn a_failure_is_shown_in_the_boxs_words() {
        let vectors: Value = serde_json::from_str(include_str!("../../../protocol/vectors/sign-in.json")).unwrap();
        for case in vectors["failures"].as_array().unwrap() {
            let status: Status = serde_json::from_value(json!({ "status": case["status"], "message": case["message"], "failure": case["failure"] })).unwrap();
            assert_eq!(status.failure_words().as_deref(), case["failure"].as_str());
        }
        let pending: Status = serde_json::from_value(json!({ "status": "pending", "message": "Downloading…" })).unwrap();
        assert_eq!(pending.failure_words(), None);
    }
}
