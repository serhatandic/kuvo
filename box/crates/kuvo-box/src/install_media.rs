//! The shell installer hands signed Rust media to the existing released-app move.

use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use kuvo_keeper::{
    mac_helper::LaunchAgent,
    manifest::{SignedRelease, UPDATE_KEY},
    staging::{self, MAX_ENVELOPE},
};
use kuvo_os::platform::{Account, Platform};
use kuvo_store::{files, home::Home, locks};
use serde_json::json;

pub fn run(args: &[OsString]) -> ExitCode {
    match move_app(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            println!(
                "{}",
                json!({"result":"failed", "message":error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

fn move_app(args: &[OsString]) -> Result<u8, Box<dyn std::error::Error>> {
    let (restart, media_args) = match args.split_last() {
        Some((last, paths)) if last == "--start" => (true, paths),
        _ => (false, args),
    };
    let [source, root, bundle] = media_args else {
        return Err(io::Error::other(
            "Usage: kuvo-box install-media <release folder> <home name> <app> [--start]",
        )
        .into());
    };
    let root = Path::new(root);
    let source = Path::new(source);
    let bundle = Path::new(bundle);
    if root.file_name() != Some(root.as_os_str()) || !source.is_absolute() || !bundle.is_absolute()
    {
        return Err(io::Error::other("The release, app or Kuvo home has an invalid path.").into());
    }

    let account = Account::current()?;
    let profile = account
        .home_folder(Platform::Mac, &std::env::vars_os().collect())?
        .canonicalize()?;
    let system = account.home.canonicalize()?;
    let target = profile.join(root);
    let located = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    let real = located(&target)
        .to_string_lossy()
        .eq_ignore_ascii_case(&located(&system.join(".kuvo-code")).to_string_lossy());
    let app_id = kuvo_move::mac::bundle_id(bundle)?;
    if real != (app_id == "run.kuvo.code") {
        return Err(io::Error::other(if real {
            "Only Kuvo itself sets up ~/.kuvo-code: a copy of it keeps a folder of its own."
        } else {
            "Kuvo itself sets up ~/.kuvo-code only."
        })
        .into());
    }
    let home = Home::new(PathBuf::from(target))?;
    let app = profile.join("Library/Application Support").join(&app_id);
    let before = kuvo_move::install::judge(&home, Some(&app), Platform::Mac, None)?;
    if restart && before.verdict == kuvo_move::install::Verdict::Ours {
        let launched = LaunchAgent::for_account(&profile, real)
            .map(|agent| agent.start(&home, &app_id))
            .transpose()?;
        println!(
            "{}",
            json!({"result":"nothing", "verdict":before.verdict, "status":before.status,
                "started":launched.is_some(),
                "message":launched.map_or("It's made, and starts by itself only in ~/.kuvo-code.", |started| started.message())})
        );
        return Ok(0);
    }
    if restart {
        println!("{}", json!({"result":"refused","verdict":before.verdict}));
        return Ok(1);
    }
    if before.verb != Some(kuvo_move::install::Verb::Move) {
        let ours = before.verdict == kuvo_move::install::Verdict::Ours;
        println!(
            "{}",
            json!({"result":if ours {"nothing"} else {"refused"}, "verdict":before.verdict})
        );
        return Ok(u8::from(!ours));
    }
    let test_key = (!real)
        .then(|| std::env::var("KUVO_TEST_RELEASE_KEY").ok())
        .flatten();
    let signed = SignedRelease::verify(
        &staging::read_release(&source.join("boxes.json"), MAX_ENVELOPE)?,
        &[test_key.as_deref().unwrap_or(UPDATE_KEY)],
    )?;
    files::create_private_dirs(&home.updates())?;
    let _lock = locks::try_file_lock(&home.updates().join(".install-media.lock"))?;
    let media = staging::prepare_install(&home, &signed, source)?;
    let mut shown = 0;
    let mut progress = |step: &serde_json::Value| {
        if let Some(index) = step["index"].as_u64().filter(|index| *index > shown) {
            shown = index;
            // The move and its journal outlive a client that stops reading.
            let _ = writeln!(io::stdout().lock(), "{step}");
        }
    };
    progress(&kuvo_move::rehome_progress("plan"));
    let move_result = kuvo_move::mac::Run {
        home: &home,
        account: &profile,
        system_account: &system,
        bundle,
        release: &media.root().join("release"),
        engine: &media.root().join("engine"),
    }
    .execute(|_| progress(&kuvo_move::rehome_status(&home)["progress"]));
    let mut value = serde_json::to_value(&move_result)?;
    if move_result.exit_code() == 0
        && value["result"] == "done"
        && let Some(agent) = LaunchAgent::for_account(&profile, real)
    {
        progress(&kuvo_move::rehome_progress("settle"));
        match agent.start(&home, &app_id) {
            Ok(message) => {
                value["started"] = true.into();
                value["message"] = message.message().into();
            }
            Err(error) => eprintln!("helper start: {error}"),
        }
    }
    value["verdict"] = serde_json::to_value(
        kuvo_move::install::judge(&home, Some(&app), Platform::Mac, None)?.verdict,
    )?;
    let _ = writeln!(io::stdout().lock(), "{value}");
    if !real && move_result.exit_code() == 0 && home.updates().join("install").is_dir() {
        files::remove_tree_synced(&home.updates().join("install"))?;
    }
    Ok(move_result.exit_code())
}
