//! File permission checks for a conversation's browser steps. These checks
//! follow links, and are also used for requests the page makes itself.
use kuvo_engine::Engine;
use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone)]
pub(super) struct Access {
    anywhere: bool,
    places: Vec<PathBuf>,
}
impl Access {
    pub async fn for_session(engine: &Engine, session: Option<&str>) -> Self {
        let finder = engine.work_engine().await;
        let mut id = session.map(str::to_owned);
        let mut found = None;
        for _ in 0..6 {
            let Some(session) = id.take() else {
                break;
            };
            let Ok((data, _)) = finder.session_found(&session).await else {
                break;
            };
            if data["id"] != session {
                break;
            }
            id = data["parentID"].as_str().map(str::to_owned);
            found = Some(data);
        }
        let anywhere = found.as_ref().is_some_and(|data| {
            !data["permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|rule| {
                    let action = rule["permission"]
                        .as_str()
                        .or_else(|| rule["action"].as_str());
                    let effect = rule["effect"].as_str().or_else(|| {
                        rule["permission"]
                            .is_string()
                            .then(|| rule["action"].as_str())
                            .flatten()
                    });
                    let resource = rule["resource"]
                        .as_str()
                        .or_else(|| rule["pattern"].as_str());
                    action == Some("shell")
                        && (effect == Some("ask")
                            || effect == Some("deny") && resource == Some("*"))
                })
        });
        let mut places = vec![std::env::temp_dir()];
        if cfg!(unix) {
            places.push(PathBuf::from("/tmp"));
        }
        if let Some(directory) = found.as_ref().and_then(|s| {
            s["location"]["directory"]
                .as_str()
                .or_else(|| s["directory"].as_str())
        }) {
            places.push(PathBuf::from(directory));
        }
        let places = places
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        Self { anywhere, places }
    }
    pub fn anywhere(&self) -> bool {
        self.anywhere
    }
    pub fn allows(&self, path: &Path) -> bool {
        self.anywhere
            || real_target(path).is_some_and(|path| self.places.iter().any(|p| path.starts_with(p)))
    }
    pub fn outside(&self, address: &str) -> Option<String> {
        if self.anywhere {
            return None;
        }
        let Ok(url) = Url::parse(address) else {
            return None;
        };
        if url.scheme() != "file" {
            return None;
        }
        match url.to_file_path() {
            Ok(path) if self.allows(&path) => None,
            Ok(path) => Some(path.to_string_lossy().into_owned()),
            Err(_) => Some(url.path().into()),
        }
    }
}
pub(super) fn real_target(path: &Path) -> Option<PathBuf> {
    let mut place = path;
    let mut parts = Vec::new();
    loop {
        if let Ok(mut real) = place.canonicalize() {
            for part in parts.iter().rev() {
                real.push(part);
            }
            return Some(real);
        }
        parts.push(place.file_name()?);
        place = place.parent()?;
    }
}
pub(super) fn outside_in_tree(tree: &Value, access: &Access) -> Option<String> {
    for address in [
        tree["frame"]["url"].as_str(),
        tree["frame"]["securityOrigin"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(file) = access.outside(address) {
            return Some(file);
        }
    }
    for resource in tree["resources"].as_array().into_iter().flatten() {
        if resource["failed"] != true
            && resource["canceled"] != true
            && let Some(file) = resource["url"].as_str().and_then(|url| access.outside(url))
        {
            return Some(file);
        }
    }
    tree["childFrames"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|child| outside_in_tree(child, access))
}
