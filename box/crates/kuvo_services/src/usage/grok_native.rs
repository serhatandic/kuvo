//! A short-lived Grok ACP process can report the account's own plan without
//! installing a full conversation adapter or making a model request.
use kuvo_os::{
    platform::{Account as OsAccount, Environment, Platform, env_value, which},
    process::{Spawn, Stdio},
};
use serde_json::{Value, json};
use std::{
    ffi::OsStr,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

pub fn folder(managed: Option<PathBuf>, env: &Environment, personal: &Path) -> PathBuf {
    if let Some(home) = managed {
        return home;
    }
    match env_value(Platform::current(), env, "GROK_HOME")
        .and_then(OsStr::to_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some("~") => personal.to_owned(),
        Some(path) if path.starts_with("~/") => personal.join(&path[2..]),
        Some(path) => PathBuf::from(path),
        None => personal.join(".grok"),
    }
}
/// Only the CLI's own saved sign-in, never browser or editor credentials.
pub fn credential(home: &Path) -> io::Result<Option<(String, Option<String>)>> {
    let Some(stored) = kuvo_store::files::read(&home.join("auth.json"))? else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_slice(&stored)?;
    Ok(parsed.as_object().and_then(|entries| {
        entries.values().find_map(|entry| {
            let key = entry["key"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            Some((
                key.to_owned(),
                entry["id_token"].as_str().map(str::to_owned),
            ))
        })
    }))
}
struct Asking {
    input: std::io::PipeWriter,
    answers: mpsc::Receiver<Value>,
    id: u64,
}
impl Asking {
    fn call(&mut self, method: &str, params: Value) -> io::Result<Result<Value, Value>> {
        self.id += 1;
        let id = self.id;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )?;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let reply = self
                .answers
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|e| match e {
                    mpsc::RecvTimeoutError::Timeout => io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Grok did not answer about its plan",
                    ),
                    mpsc::RecvTimeoutError::Disconnected => io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Grok stopped before its answer",
                    ),
                })?;
            if reply["id"] != id {
                continue;
            }
            if let Some(result) = reply.get("result") {
                return Ok(Ok(result.clone()));
            }
            return Ok(Err(reply["error"].clone()));
        }
    }
}
fn auth_error(error: &Value) -> bool {
    error["message"]
        .as_str()
        .is_some_and(|s| s.to_ascii_lowercase().contains("auth"))
        || error["data"]
            .as_str()
            .is_some_and(|s| s.to_ascii_lowercase().contains("auth"))
}
pub enum Answer {
    Billing(Value),
    Expired,
    Failed,
}
pub async fn read(home: PathBuf, isolated: bool) -> io::Result<Answer> {
    tokio::task::spawn_blocking(move|| {
        let mut env:Environment=std::env::vars_os().collect();
        let personal=OsAccount::current()?.home_folder(Platform::current(),&env)?;
        let extras=if isolated{vec![]}else{vec![personal.join(".grok/bin"),personal.join(".local/bin"),PathBuf::from("/opt/homebrew/bin"),PathBuf::from("/usr/local/bin")]};
        let executable=which(OsStr::new("grok"),&env,&extras).ok_or_else(||io::Error::new(io::ErrorKind::NotFound,"Grok's CLI isn't installed here."))?;
        env.retain(|name,_|!name.eq_ignore_ascii_case("GROK_HOME"));
        env.insert("GROK_HOME".into(),home.as_os_str().to_owned());
        env.insert("GROK_DISABLE_AUTOUPDATER".into(),"1".into());
        let mut spawn=Spawn::new(executable.path(),env);
        spawn.executable=executable;spawn.cwd=Some(home);
        spawn.args=["agent","--no-leader","stdio"].into_iter().map(Into::into).collect();
        spawn.stdin=Stdio::Pipe;spawn.stdout=Stdio::Pipe;spawn.stderr=Stdio::Null;
        let mut child=spawn.spawn()?;
        let input=child.stdin.take().expect("Grok stdin");
        let output=child.stdout.take().expect("Grok stdout");
        let (sender,answers)=mpsc::sync_channel(8);
        let reader=std::thread::Builder::new().name("grok-usage".into()).spawn(move||{
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                let Ok(reply)=serde_json::from_str::<Value>(&line)else{continue};
                if reply.get("id").is_some()&&sender.send(reply).is_err(){break;}
            }
        })?;
        let mut asking=Asking{input,answers,id:0};
        let result=(||{
            match asking.call("initialize",json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"kuvo-code","title":"Kuvo Code","version":"1"},"_meta":{"clientType":"extension"}}))? {
                Ok(_)=>{},Err(error)=>return Ok(if auth_error(&error){Answer::Expired}else{Answer::Failed})
            }
            let mut billing=asking.call("_x.ai/billing",json!({}))?;
            if billing.as_ref().err().is_some_and(auth_error) {
                match asking.call("authenticate",json!({"methodId":"cached_token"}))? {
                    Ok(_)=>{},Err(error)=>return Ok(if auth_error(&error){Answer::Expired}else{Answer::Failed})
                }
                billing=asking.call("_x.ai/billing",json!({}))?;
            }
            Ok(match billing{Ok(body)=>Answer::Billing(body),Err(e) if auth_error(&e)=>Answer::Expired,Err(_)=>Answer::Failed})
        })();
        drop(asking);
        child.stop(Duration::from_secs(1), |_|{})?;
        let _=reader.join();
        result
    }).await?
}
