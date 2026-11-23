use super::{engine, invalid};
use crate::{Error, metadata};
use kuvo_store::{files, sqlite};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::{collections::HashSet, path::Path, time::Duration};
use url::Url;

#[derive(Deserialize)]
struct SessionPage {
    data: Vec<Entry>,
    cursor: Option<Cursor>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
}

#[derive(Deserialize)]
struct Cursor {
    next: Option<String>,
}

pub(super) fn validate(engine: &Path, data: &Path, scratch: &Path) -> Result<(), Error> {
    let database = data.join("opencode/opencode.db");
    if metadata(&database)?.is_none() {
        return Ok(());
    }
    let (count, sample) = {
        let db = sqlite::open_index(&database, Duration::from_secs(3))?;
        if engine::schema(&db)?.is_none() {
            return Ok(());
        }
        let expected: usize =
            db.query_row("SELECT count(*) FROM session_v2", [], |row| row.get(0))?;
        if expected == 0 {
            return Ok(());
        }
        let mut query = db.prepare(
            "SELECT id, directory FROM session_v2 ORDER BY time_updated DESC, id LIMIT 25",
        )?;
        let sample = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        (expected, sample)
    };
    let clone = scratch.join("data");
    let copy = clone.join("opencode/opencode.db");
    files::clone_mac_entry(&database, &copy, true)?;
    engine::with_engine(engine, &clone, &scratch.join("check"), |url, password| {
        compare(url, password, count, &sample)
    })
}

fn compare(
    base: Url,
    password: &str,
    count: usize,
    sample: &[(String, String)],
) -> Result<(), Error> {
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(io_error)?;
    let get = |url| {
        client
            .get(url)
            .basic_auth("opencode", Some(password))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(io_error)
    };
    let mut listed = HashSet::new();
    for archived in [false, true] {
        let mut cursors = HashSet::new();
        let mut next: Option<String> = None;
        loop {
            let mut url = base.join("/api/session").map_err(io_error)?;
            url.query_pairs_mut()
                .append_pair("limit", "200")
                .append_pair("order", "desc");
            if archived {
                url.query_pairs_mut().append_pair("archived", "true");
            }
            if let Some(cursor) = next.take() {
                url.query_pairs_mut().append_pair("cursor", &cursor);
            }
            let page: SessionPage = get(url)?.json().map_err(io_error)?;
            if page.data.is_empty() {
                break;
            }
            listed.extend(page.data.into_iter().map(|row| row.id));
            next = page
                .cursor
                .and_then(|cursor| cursor.next)
                .filter(|cursor| !cursor.is_empty());
            let Some(cursor) = &next else {
                break;
            };
            if !cursors.insert(cursor.clone()) || cursors.len() >= 100_000 {
                return Err(invalid(
                    "The staged engine did not finish listing sessions.",
                ));
            }
        }
    }
    if listed.len() != count {
        return Err(invalid(&format!(
            "The staged engine lists {} sessions instead of {count}.",
            listed.len()
        )));
    }
    for (id, directory) in sample {
        let mut url = base.join("/api/session").map_err(io_error)?;
        url.path_segments_mut()
            .map_err(|_| invalid("The staged engine has an invalid address."))?
            .push(id);
        let reply: serde_json::Value = get(url)?.json().map_err(io_error)?;
        let found = reply
            .get("data")
            .unwrap_or(&reply)
            .get("location")
            .and_then(|value| value.get("directory"))
            .and_then(serde_json::Value::as_str);
        if found != Some(directory) {
            return Err(invalid(
                "The staged engine reported a different session directory.",
            ));
        }
    }
    Ok(())
}

fn io_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    std::io::Error::other(error).into()
}
