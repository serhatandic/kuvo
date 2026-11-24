//! The shell installer hands signed Rust media to the existing released-app move.

use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

use kuvo_keeper::{
    mac_helper::LaunchAgent,
    manifest::{SignedRelease, UPDATE_KEY},
    staging::{self, MAX_ENVELOPE},
};
use kuvo_move::install::{self, Verb, Verdict};
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
    let real = install::same_mac_home(&target, &system.join(".kuvo-code"));
    let app_id = kuvo_move::mac::bundle_id(bundle)?;
    if real != (app_id == "run.kuvo.code") {
        return Err(io::Error::other(if real {
            "Only Kuvo itself sets up ~/.kuvo-code: a copy of it keeps a folder of its own."
        } else {
            "Kuvo itself sets up ~/.kuvo-code only."
        })
        .into());
    }
    let home = Home::new(target)?;
    let app = profile.join("Library/Application Support").join(&app_id);
    let before = install::judge(&home, Some(&app), Platform::Mac, None)?;
    if let Some(message) = before.unreadable_move() {
        println!("{}", json!({"result":"refused", "message":message}));
        return Ok(1);
    }
    let declined = match (restart, before.verdict) {
        (_, Verdict::Waits) => Some((4, "waits", "Another move is under way.")),
        (false, Verdict::Machine) => Some((
            1,
            "refused",
            "This Mac is set up as one of your machines already: Kuvo uses what's here.",
        )),
        (true, Verdict::Machine) => Some((
            1,
            "refused",
            "This computer is set up as one of your machines already: Kuvo uses what's here.",
        )),
        (false, Verdict::Two) => Some((1, "refused", install::TWO_HOMES_MOVE)),
        (true, Verdict::Two) => Some((
            1,
            "refused",
            "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: both stay as they are.",
        )),
        (true, Verdict::App07 | Verdict::AppBox) => Some((
            1,
            "refused",
            "Kuvo Code has your sessions and settings on this Mac already: they move into their new place instead.",
        )),
        (false, Verdict::Fresh) => Some((
            1,
            "refused",
            "There's nothing of an earlier Kuvo Code's on this Mac to move.",
        )),
        (_, Verdict::Back) => Some((
            1,
            "refused",
            "Kuvo Code's sessions on this Mac were moved back: they move again when you ask.",
        )),
        (true, Verdict::Moving) => Some((
            1,
            "refused",
            "Kuvo is moving your sessions to their new place on this Mac: that's finished first.",
        )),
        (true, Verdict::Stuck) => Some((
            1,
            "refused",
            "Kuvo was moving your sessions to their new place on this Mac and stopped part way: that's finished first.",
        )),
        (true, Verdict::Fresh | Verdict::Ours)
        | (
            false,
            Verdict::Ours | Verdict::App07 | Verdict::AppBox | Verdict::Moving | Verdict::Stuck,
        ) => None,
    };
    if let Some((code, result, message)) = declined {
        println!(
            "{}",
            json!({"result":result, "verdict":before.verdict, "message":message, "status":before.status})
        );
        return Ok(code);
    }
    let verified = || -> io::Result<(SignedRelease, Vec<u8>)> {
        let key = (!real)
            .then(|| std::env::var("KUVO_TEST_RELEASE_KEY").ok())
            .flatten();
        let envelope = staging::read_release(&source.join("boxes.json"), MAX_ENVELOPE)?;
        let signed = SignedRelease::verify(&envelope, &[key.as_deref().unwrap_or(UPDATE_KEY)])?;
        Ok((signed, envelope))
    };
    if restart && before.verdict == Verdict::Ours {
        files::create_private_dirs(&home.updates())?;
        let _lock = locks::try_file_lock(&home.updates().join(".install-media.lock"))?;
        if fs::symlink_metadata(home.bin()).is_ok_and(|entry| entry.is_dir())
            && home.bin().join("kuvo-box").is_file()
        {
            let (signed, envelope) = verified()?;
            staging::select_install(&home, &signed, source, &envelope)?;
        }
        let launched = LaunchAgent::for_account(&profile, real)
            .map(|agent| agent.start(&home, &app_id))
            .transpose()?;
        files::remove_tree_synced(&home.updates().join("install"))?;
        println!(
            "{}",
            json!({"result":"nothing", "verdict":before.verdict, "status":before.status,
                "started":launched.is_some(),
                "message":launched.map_or("It's made, and starts by itself only in ~/.kuvo-code.", |started| started.message())})
        );
        return Ok(0);
    }
    if !restart && before.verb != Some(Verb::Move) {
        let ours = before.verdict == Verdict::Ours;
        println!(
            "{}",
            json!({"result":if ours {"nothing"} else {"refused"}, "verdict":before.verdict})
        );
        return Ok(u8::from(!ours));
    }
    let (signed, envelope) = verified()?;
    files::create_private_dirs(&home.updates())?;
    let _lock = locks::try_file_lock(&home.updates().join(".install-media.lock"))?;
    let media = staging::prepare_install(&home, &signed, source)?;
    if restart {
        let (result, set_aside) =
            kuvo_move::mac::new_home_from_media(&home, &app.join("box"), &system, &media);
        let mut value = serde_json::to_value(&result)?;
        if let Some(path) = set_aside {
            value["setAside"] = serde_json::to_value(path)?;
        }
        let code = result.exit_code();
        let after = install::judge(&home, Some(&app), Platform::Mac, None)?;
        value["verdict"] = serde_json::to_value(after.verdict)?;
        if code == 0 {
            staging::select_install(&home, &signed, source, &envelope)?;
            match LaunchAgent::for_account(&profile, real) {
                Some(agent) => match agent.start(&home, &app_id) {
                    Ok(started) => {
                        value["started"] = true.into();
                        value["message"] = started.message().into();
                    }
                    Err(error) => {
                        value["result"] = "failed".into();
                        value["started"] = false.into();
                        value["message"] = error.to_string().into();
                        println!("{value}");
                        return Ok(1);
                    }
                },
                None => {
                    value["started"] = false.into();
                    value["message"] =
                        "It's made, and starts by itself only in ~/.kuvo-code.".into();
                }
            }
            files::remove_tree_synced(&home.updates().join("install"))?;
        }
        println!("{value}");
        return Ok(code);
    }
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
    if move_result.exit_code() == 0 && value["result"] == "done" {
        staging::select_install(&home, &signed, source, &envelope)?;
        if let Some(agent) = LaunchAgent::for_account(&profile, real) {
            progress(&kuvo_move::rehome_progress("settle"));
            match agent.start(&home, &app_id) {
                Ok(message) => {
                    value["started"] = true.into();
                    value["message"] = message.message().into();
                }
                Err(error) => eprintln!("helper start: {error}"),
            }
        }
    }
    value["verdict"] =
        serde_json::to_value(install::judge(&home, Some(&app), Platform::Mac, None)?.verdict)?;
    let _ = writeln!(io::stdout().lock(), "{value}");
    if !real && move_result.exit_code() == 0 && home.updates().join("install").is_dir() {
        files::remove_tree_synced(&home.updates().join("install"))?;
    }
    Ok(move_result.exit_code())
}
