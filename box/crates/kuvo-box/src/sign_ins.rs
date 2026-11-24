//! The desktop explicitly asks the old app to hand its saved sign-ins over.
use std::{
    env, fs, io,
    path::Path,
    sync::{Arc, Mutex},
};

use http::StatusCode;
use kuvo_os::{platform::Account, process::Spawn};
use kuvo_protocol::routes::{get_kuvo_box_rehome as status, post_kuvo_box_rehome as start};
use kuvo_server::{Access, Caller, LocalOrigin, Refusal, Registry, RouteReply, Transport};
use kuvo_store::{files, home::Home};
use serde_json::Value;

pub fn register(routes: &mut Registry, home: Home) {
    let queried = home.clone();
    routes.register::<status::Spec, _, _>(Access::Any, move |_| {
        let home = queried.clone();
        async move {
            let status = tokio::task::spawn_blocking(move || kuvo_move::rehome_status(&home))
                .await
                .map_err(|_| failed())?;
            Ok(RouteReply::json::<200>(
                serde_json::from_value(status).map_err(|_| failed())?,
            ))
        }
    });
    let starting = Arc::new(Mutex::new(()));
    routes.register::<start::Spec, _, _>(Access::Any, move |call| {
        let home = home.clone();
        let starting = starting.clone();
        async move {
            if !matches!(
                call.request.caller,
                Caller::Local(Transport::Http {
                    origin: LocalOrigin::App,
                    ..
                })
            ) {
                return Err(Refusal::new(
                    StatusCode::FORBIDDEN,
                    "Your saved sign-ins come over from Kuvo on the Mac they were saved on.",
                ));
            }
            let body = serde_json::from_slice::<Value>(&call.request.body).unwrap_or_default();
            if body["signIns"] != true {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    "Say Move Now (now) or Move Back (back).",
                ));
            }
            let (started, status) = tokio::task::spawn_blocking(move || {
                let _guard = starting.lock().map_err(|_| failed())?;
                launch(&home)
            })
            .await
            .map_err(|_| failed())??;
            let status = serde_json::from_value(status).map_err(|_| failed())?;
            Ok(if started {
                RouteReply::json::<202>(status)
            } else {
                RouteReply::json::<200>(status)
            })
        }
    });
}

fn launch(home: &Home) -> Result<(bool, Value), Refusal> {
    let system = Account::current().map_err(|_| failed())?.home;
    if !kuvo_move::mac::owns_sign_ins_home(home, &system) {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "There are no saved sign-ins to bring over on this computer.",
        ));
    }
    let status = kuvo_move::rehome_status(home);
    if status["signIns"]["state"] == "asking" {
        return Ok((false, status));
    }
    if status["state"] != "done"
        || status["way"] != "move"
        || !status["owed"]
            .as_array()
            .is_some_and(|owed| owed.iter().any(|part| part == "passwords"))
    {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "There are no saved sign-ins left to bring over.",
        ));
    }
    let executable = env::current_exe().map_err(|_| failed())?;
    let mut launch = Spawn::new(executable, env::vars_os().collect());
    launch.cwd = Some(home.root().to_owned());
    launch.args = ["rehome".into(), ".kuvo-code".into(), "--sign-ins".into()].into();
    let mut child = launch
        .spawn_recoverable("kuvo-saved-site-transfer")
        .map_err(|_| failed())?;
    let pid = child.identity().pid.to_string();
    let mark = home.root().join("updates/rehome/sign-ins.pid");
    files::write_secret(&mark, pid.as_bytes()).map_err(|_| failed())?;
    child.leave_running_on_drop();
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = remove_own_mark(&mark, &pid);
    });
    Ok((true, kuvo_move::rehome_status(home)))
}

fn remove_own_mark(mark: &Path, pid: &str) -> io::Result<()> {
    if fs::read_to_string(mark).is_ok_and(|kept| kept.trim() == pid) {
        fs::remove_file(mark)?;
    }
    Ok(())
}

fn failed() -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "Kuvo couldn't start bringing your saved sign-ins over. Try again.",
    )
}
