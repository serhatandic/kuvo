use std::{io, path::Path, time::Duration};

use kuvo_server::ServeRecord;
use kuvo_store::files;
use serde_json::Value;

pub(crate) fn probe_client() -> io::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(io::Error::other)
}

/// Readiness belongs to the resident that published this generation. An old listener
/// on the same port, an earlier resident, or a server still binding does not pass.
pub async fn resident_info(home: &Path, pid: u32) -> io::Result<Option<Value>> {
    probe(&probe_client()?, &home.join("run"), pid).await
}

pub(crate) async fn probe(
    client: &reqwest::Client,
    run: &Path,
    pid: u32,
) -> io::Result<Option<Value>> {
    let Some(record) = files::read(&run.join("serve.json"))?
        .and_then(|bytes| serde_json::from_slice::<ServeRecord>(&bytes).ok())
        .filter(|record| record.pid == pid)
    else {
        return Ok(None);
    };
    let Some(build) = files::read(&run.join("link.version"))? else {
        return Ok(None);
    };
    let endpoint = format!("http://127.0.0.1:{}/kuvo/info", record.port);
    let Ok(reply) = client
        .get(endpoint)
        .basic_auth("opencode", Some(&record.password))
        .send()
        .await
    else {
        return Ok(None);
    };
    if reply.status() != 200 {
        return Ok(None);
    }
    let Ok(body) = reply.json::<Value>().await else {
        return Ok(None);
    };
    let generation = record.generation.to_string();
    Ok((body["serverGeneration"].as_str() == Some(&generation)
        && body["serverBuild"]
            .as_str()
            .is_some_and(|value| value.as_bytes() == build))
    .then_some(body))
}
