//! `GET kuvo/programs`: checks whether this box has the program an MCP server's command names.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use http::StatusCode;
use kuvo_protocol::{
    json::{Null, Nullable},
    routes::get_kuvo_programs as route,
};
use kuvo_server::{Access, Refusal, Registry, RouteReply};

pub const FEATURES: &[&str] = &["programs"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramFound {
    pub name: String,
    pub path: Option<String>,
    pub unknown: bool,
}

pub fn register(routes: &mut Registry, home: PathBuf) {
    routes.register::<route::Spec, _, _>(Access::Any, move |call| {
        let query = call.request.path.split_once('?').map_or("", |(_, q)| q);
        let name = form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let path_env = std::env::var_os("PATH").unwrap_or_default();
        let home = home.clone();
        async move {
            match find_program(&name, &path_env, &home) {
                Ok(found) => Ok(RouteReply::json::<200>(route::Status200 {
                    name: found.name,
                    path: found.path.map_or(Nullable::Null(Null), Nullable::Value),
                    unknown: found.unknown.then_some(true),
                })),
                Err(msg) => Err(Refusal::new(StatusCode::BAD_REQUEST, msg)),
            }
        }
    });
}

pub fn find_program(
    raw_name: &str,
    path_env: &OsString,
    account_home: &Path,
) -> Result<ProgramFound, &'static str> {
    let name = raw_name.trim();
    if name.is_empty() {
        return Err("Name the program (name).");
    }
    if name.starts_with("./")
        || name.starts_with("../")
        || name.contains('$')
        || (name.starts_with('~') && !name.starts_with("~/") && name.len() > 1)
    {
        return Ok(ProgramFound { name: name.into(), path: None, unknown: true });
    }
    let found = if name.contains('/') || (cfg!(windows) && name.contains('\\')) {
        let file = name
            .strip_prefix("~/")
            .map_or_else(|| PathBuf::from(name), |rest| account_home.join(rest));
        file.is_absolute().then(|| find_runnable(&file)).flatten()
    } else {
        std::env::split_paths(path_env)
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| find_runnable(&dir.join(name)))
    };
    Ok(ProgramFound {
        name: name.into(),
        path: found.map(|p| p.to_string_lossy().into_owned()),
        unknown: false,
    })
}

#[cfg(unix)]
fn find_runnable(file: &Path) -> Option<PathBuf> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let c_path = CString::new(file.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c_path` is NUL-terminated.
    let ok = unsafe { libc::access(c_path.as_ptr(), libc::X_OK) == 0 };
    (ok && fs::metadata(file).is_ok_and(|m| m.is_file())).then(|| file.to_path_buf())
}

#[cfg(windows)]
fn find_runnable(file: &Path) -> Option<PathBuf> {
    if fs::metadata(file).is_ok_and(|m| m.is_file()) {
        return Some(file.to_path_buf());
    }
    if file.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let mut candidate = file.as_os_str().to_os_string();
            candidate.push(ext);
            let path = PathBuf::from(candidate);
            if fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(not(any(unix, windows)))]
fn find_runnable(file: &Path) -> Option<PathBuf> {
    fs::metadata(file).is_ok_and(|m| m.is_file()).then(|| file.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_runnable_programs_and_marks_relative_or_shell_variables_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let prog = bin.join("my-tool");
        fs::write(&prog, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&prog, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path_env = std::env::join_paths([&bin]).unwrap();
        assert_eq!(find_program("", &path_env, dir.path()), Err("Name the program (name)."));
        let found = find_program("my-tool", &path_env, dir.path()).unwrap();
        assert_eq!(found.path.as_deref(), Some(prog.to_string_lossy().as_ref()));
        assert!(!found.unknown);
        assert_eq!(find_program("definitely-absent-tool", &path_env, dir.path()).unwrap().path, None);
        for name in ["./server", "../server", "$HOME/server", "~other/server"] {
            let res = find_program(name, &path_env, dir.path()).unwrap();
            assert!(res.unknown && res.path.is_none(), "{name}");
        }
        let tilde = find_program("~/bin/my-tool", &path_env, dir.path()).unwrap();
        assert_eq!(tilde.path.as_deref(), Some(prog.to_string_lossy().as_ref()));
    }
}
