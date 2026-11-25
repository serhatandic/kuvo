//! Released beta.1 Macs' SSH settings verbs. Use the same settings writer as
//! the routes and the same adoption as the one-time machine move.
use std::{
    io::{self, Read},
    process::ExitCode,
    time::Duration,
};

use kuvo_services::config::{ConfigPaths, SettingsChanged, SettingsRead, is_key_name, shown_root};
use kuvo_store::{files, home::Home, locks::OwnedLock};
use serde_json::{Value, json};

pub fn run(
    home: &Home,
    verb: &str,
    key: Option<&str>,
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mac =
        files::text_of(&home.settings().join("kind")).is_some_and(|kind| kind.trim() == "mac");
    let config = ConfigPaths::for_home(home.root(), mac);
    let mut input = String::new();
    if verb != "read" {
        io::stdin().read_to_string(&mut input)?;
    }
    let answer = match verb {
        "read" => match config.read_settings() {
            SettingsRead::Ok(settings) => {
                json!({"status":200,"settings":settings,"own":config.owns_settings()})
            }
            SettingsRead::Problem(message) => json!({"status":409,"message":message}),
        },
        "adopt" if mac => {
            let settings = match config.read_settings() {
                SettingsRead::Ok(settings) => settings,
                SettingsRead::Problem(_) => Default::default(),
            };
            json!({"status":"own","settings":settings})
        }
        "adopt" => {
            let seed = if input.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&input)?
            };
            kuvo_move::adopt_legacy_settings(home.root(), &shown_root(home.root()), &seed)?
        }
        "change" if !config.owns_settings() => {
            json!({"status":409,"message":"This machine's settings aren't its own yet."})
        }
        "change" => {
            let given: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
            match config.change_settings(&given["changes"]) {
                SettingsChanged::Ok(settings) => json!({"status":200,"settings":settings}),
                SettingsChanged::Err {
                    status,
                    message,
                    problem,
                } => {
                    let mut reply = json!({"status":status,"message":message});
                    if let Some(problem) = problem {
                        reply["problem"] = json!(problem);
                    }
                    reply
                }
            }
        }
        "key" => {
            let name = key.unwrap_or_default();
            if !is_key_name(name) || name.contains("__") {
                json!({"status":400,"message":"That isn't a key's name."})
            } else {
                let _lock = OwnedLock::acquire(&config.lock_file, Duration::from_secs(15))?;
                let file = home.settings().join("keys").join(name);
                if input.is_empty() {
                    files::remove_file_synced(&file)?;
                } else {
                    files::write_secret(&file, input.as_bytes())?;
                }
                json!({"status":200})
            }
        }
        "personalities" => personalities(home, &input)?,
        _ => unreachable!(),
    };
    println!("{answer}");
    Ok(ExitCode::SUCCESS)
}

/// beta.1 merges its library with the machine's over SSH. The resident owns the library and
/// its setup save, so the command is handed to it as an internal route, answered as before:
/// the library, with the save beside it.
fn personalities(home: &Home, input: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let given: Value = serde_json::from_str(input).unwrap_or(Value::Null);
    let request = json!({"method": "POST", "path": kuvo_setup::PERSONALITY_EXCHANGE, "body": given});
    let mut output = Vec::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(kuvo_server::ssh::pipe(
            home.root(),
            kuvo_server::ssh::Command::Route {
                internal: true,
                internal_path: None,
            },
            request.to_string().as_bytes(),
            &mut output,
        ))?;
    let line = output
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let routed: Value = serde_json::from_slice(line)?;
    let mut answer = routed["body"]
        .as_str()
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    answer["status"] = routed["status"].clone();
    Ok(answer)
}
