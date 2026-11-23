use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use kuvo_os::process::legacy_process_runs;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{Grant, How, Item, Secret, Who};

#[derive(Default)]
pub(super) struct Identity {
    pub session: String,
    pub root: String,
    pub name: Option<String>,
    pub folder: Option<PathBuf>,
    pub bot: Option<Bot>,
    pub seat: bool,
    pub group: Option<String>,
    pub autopilot: bool,
}

pub(super) struct Bot {
    pub id: String,
    pub name: String,
    pub session: String,
}

#[derive(Clone)]
pub(super) struct Reader {
    http: reqwest::Client,
    home: PathBuf,
    bots: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    port: u16,
    password: Secret,
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    format_version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    id: String,
    title: Option<String>,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    location: Option<Location>,
    directory: Option<PathBuf>,
    metadata: Option<Metadata>,
}

#[derive(Deserialize)]
struct Location {
    directory: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    kuvo: Map<String, Value>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Vec<Block>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    #[serde(rename = "callID")]
    call_id: Option<String>,
    name: Option<String>,
    state: Option<ToolState>,
}

#[derive(Deserialize)]
struct ToolState {
    #[serde(default)]
    status: String,
    input: Option<Value>,
}

impl Reader {
    pub fn new(home: &Path, bots: &Path) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("vault loopback HTTP client");
        Self {
            home: home.into(),
            http,
            bots: bots.into(),
        }
    }

    async fn targets(&self) -> Vec<Target> {
        let run = self.home.join("run");
        tokio::task::spawn_blocking(move || {
            let read =
                |file| serde_json::from_slice::<Target>(&fs::read(run.join(file)).ok()?).ok();
            let engine = read("engine.json").filter(|record| record.port != 0);
            let bridge = read("bridge.json").filter(|record| {
                record.port != 0
                    && !record.password.expose().is_empty()
                    && record.format_version <= 1
                    && legacy_process_runs(record.pid, "kuvo-agents")
            });
            engine.into_iter().chain(bridge).collect()
        })
        .await
        .unwrap_or_default()
    }

    async fn at<T: serde::de::DeserializeOwned>(&self, target: &Target, path: &str) -> Option<T> {
        #[derive(Deserialize)]
        struct Answer<T> {
            data: T,
        }
        let response = self
            .http
            .get(format!("http://127.0.0.1:{}/{path}", target.port))
            .basic_auth("opencode", Some(target.password.expose()))
            .header("x-kuvo-route-version", "1")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        if response
            .headers()
            .get("x-kuvo-route-version")
            .is_some_and(|version| version != "1")
        {
            return None;
        }
        Some(response.json::<Answer<T>>().await.ok()?.data)
    }

    async fn session(&self, targets: &[Target], id: &str) -> Option<Session> {
        let id = percent_encoding::utf8_percent_encode(id, percent_encoding::NON_ALPHANUMERIC);
        let route = format!("api/session/{id}");
        for target in targets {
            if let Some(found) = self
                .at::<Session>(target, &route)
                .await
                .filter(|session| !session.id.is_empty())
            {
                return Some(found);
            }
        }
        None
    }

    pub async fn caller(&self, id: &str) -> Identity {
        if !session_id(id) {
            return Identity::default();
        }
        let targets = self.targets().await;
        let mut session = self.session(&targets, id).await;
        for _ in 0..5 {
            let Some(above) = session.as_ref().and_then(Session::above) else {
                break;
            };
            if let Some(root) = self.session(&targets, above).await {
                session = Some(root);
            }
        }
        let Some(session) = session else {
            return Identity {
                session: id.into(),
                root: id.into(),
                ..Identity::default()
            };
        };
        let unsure = session.above().is_some();
        let kuvo = session.metadata.as_ref().map(|value| &value.kuvo);
        let seat = kuvo
            .and_then(|notes| notes.get("seat"))
            .filter(|seat| !seat.is_null() && *seat != &Value::Bool(false));
        let group = seat
            .and_then(|seat| seat.get("group"))
            .and_then(Value::as_str)
            .filter(|group| !group.is_empty())
            .map(str::to_owned);
        let seat_bot = seat
            .and_then(|seat| seat.get("bot"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let work_bot = kuvo
            .and_then(|notes| notes.get("by"))
            .filter(|by| by.get("name").is_some_and(Value::is_string))
            .and_then(|by| by.get("bot"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let autopilot = kuvo
            .and_then(|notes| notes.get("autopilot"))
            .and_then(Value::as_bool)
            == Some(true);
        let bots = self.bots.clone();
        let root = session.id.clone();
        let is_seat = seat.is_some();
        let bot = tokio::task::spawn_blocking(move || {
            if is_seat {
                return seat_bot.as_deref().and_then(|id| Bot::read(&bots, id));
            }
            fs::read_dir(&bots)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| Bot::read(&bots, entry.file_name().to_str()?))
                .find(|bot| bot.session == root)
                .or_else(|| work_bot.as_deref().and_then(|id| Bot::read(&bots, id)))
        })
        .await
        .unwrap_or_default();
        Identity {
            session: id.into(),
            root: session.id,
            name: session.title.filter(|title| !title.is_empty()),
            folder: session
                .location
                .and_then(|location| location.directory)
                .or(session.directory)
                .filter(|folder| !folder.as_os_str().is_empty())
                .map(normalize),
            bot,
            seat: is_seat || unsure,
            group,
            autopilot,
        }
    }

    #[cfg(unix)]
    pub async fn folder_bot(&self, folder: &Path) -> Option<Bot> {
        let bots = self.bots.clone();
        let folder = folder.to_owned();
        tokio::task::spawn_blocking(move || {
            let id = folder
                .strip_prefix(&bots)
                .ok()?
                .components()
                .next()?
                .as_os_str()
                .to_str()?;
            Bot::read(&bots, id)
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn calling(&self, id: &str, tools: &[&str], args: Option<&Value>) -> Option<String> {
        if !session_id(id) {
            return None;
        }
        let route = format!("api/session/{id}/message?limit=6&order=desc");
        for target in self.targets().await {
            let Some(page) = self.at::<Vec<Message>>(&target, &route).await else {
                continue;
            };
            for (at, message) in page.into_iter().enumerate() {
                for (place, block) in message.content.into_iter().enumerate() {
                    if block.kind.as_deref() != Some("tool")
                        || !block.name.is_some_and(|name| {
                            tools.iter().any(|tool| {
                                name == *tool
                                    || name
                                        .strip_suffix(tool)
                                        .is_some_and(|prefix| prefix.ends_with('_'))
                            })
                        })
                    {
                        continue;
                    }
                    let Some(state) = block
                        .state
                        .filter(|state| matches!(state.status.as_str(), "running" | "pending"))
                    else {
                        continue;
                    };
                    if args.is_none_or(|args| {
                        json_equal(
                            args,
                            &state.input.unwrap_or_else(|| Value::Object(Map::new())),
                        )
                    }) {
                        let call = block
                            .call_id
                            .or(block.id)
                            .unwrap_or_else(|| format!("{at}.{place}"));
                        return Some(format!("{id}\0{call}"));
                    }
                }
            }
        }
        None
    }
}

impl Session {
    fn above(&self) -> Option<&str> {
        self.parent_id
            .as_deref()
            .or_else(|| self.metadata.as_ref()?.kuvo.get("engineFor")?.as_str())
            .filter(|above| !above.is_empty())
    }
}

impl Bot {
    pub fn read(bots: &Path, id: &str) -> Option<Self> {
        if !id
            .strip_prefix("bot_")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            return None;
        }
        #[derive(Deserialize)]
        struct Description {
            kuvo: u64,
            id: String,
            name: String,
        }
        let folder = bots.join(id);
        let description: Description =
            serde_json::from_slice(&fs::read(folder.join("bot.json")).ok()?).ok()?;
        let session = fs::read_to_string(folder.join("session")).ok()?;
        let session = session.trim();
        if description.kuvo != 1 || description.id != id || session.is_empty() {
            return None;
        }
        Some(Self {
            id: description.id,
            name: description.name,
            session: session.into(),
        })
    }
}

impl Identity {
    pub fn grant(&self, item: &Item, grants: &[Grant], even_seat: bool) -> Option<How> {
        let mut how = None;
        for grant in grants.iter().filter(|grant| grant.item == item.id) {
            let fits = match &grant.who {
                Who::Everyone { everyone } => *everyone,
                Who::Bot { bot } => self.bot.as_ref().is_some_and(|you| you.id == *bot),
                Who::Project { project } => self.folder.as_ref().is_some_and(|folder| {
                    folder == Path::new(project) || project != "/" && folder.starts_with(project)
                }),
            };
            if fits {
                if grant.how == How::Own && (!self.seat || even_seat) {
                    return Some(How::Own);
                }
                how = Some(How::Ask);
            }
        }
        how
    }

    pub fn always_for(&self, account: &Path) -> Option<Who> {
        if self.seat {
            return None;
        }
        if let Some(bot) = &self.bot {
            return Some(Who::Bot {
                bot: bot.id.clone(),
            });
        }
        let folder = self
            .folder
            .as_ref()
            .filter(|folder| *folder != account && **folder != Path::new("/"))?;
        Some(Who::Project {
            project: folder.to_str()?.into(),
        })
    }
}

fn session_id(id: &str) -> bool {
    (1..=100).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn normalize(path: PathBuf) -> PathBuf {
    let path = std::path::absolute(&path).unwrap_or(path);
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => (),
            part => out.push(part.as_os_str()),
        }
    }
    out
}

fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|b| json_equal(value, b)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_equal(a, b))
        }
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        _ => a == b,
    }
}
