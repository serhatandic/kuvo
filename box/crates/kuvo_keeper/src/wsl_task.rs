//! Upgrade the existing WSL login task's launcher, retaining its principal,
//! trigger and settings. Do not register a task for an unrelated installation.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_os::platform::{Account, Platform};
use kuvo_store::{files, home::Home};
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const FLAGS: &str = "-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand";

pub async fn repair_existing(home: &Home) -> io::Result<()> {
    if Platform::current() != Platform::Wsl
        || crate::shell_keeper(&home.bin())
            .file_name()
            .is_none_or(|name| name != ".kuvo-start-keeper")
    {
        return Ok(());
    }
    let Some(task) = files::text_of(&home.run().join("wsl-task")) else {
        return Ok(());
    };
    let task = task.trim();
    let Some(distro) = std::env::var("WSL_DISTRO_NAME").ok().filter(|name| {
        !name.is_empty()
            && name.len() <= 80
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    }) else {
        return Err(io::Error::other("The WSL distribution is unknown."));
    };
    let profile = Account::current()?.home_folder(Platform::Wsl, &std::env::vars_os().collect())?;
    if home.root() != profile.join(".kuvo-code") {
        return Err(io::Error::other(
            "The WSL task does not belong to this home.",
        ));
    }
    let powershell = output(
        Path::new("/usr/bin/wslpath"),
        &[
            "-u",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ],
    )
    .await?;
    let powershell = PathBuf::from(powershell.trim());
    let sid = windows(
        &powershell,
        "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
    )
    .await?;
    let sid = sid.trim();
    if !sid.starts_with("S-") || !sid[2..].bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return Err(io::Error::other(
            "The Windows account could not be identified.",
        ));
    }
    let identity = serde_json::to_vec(&[
        sid,
        &distro,
        home.root()
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid WSL home"))?,
    ])?;
    if task != format!("KuvoCode-WSL-{}", &crate::manifest::sha256(&identity)[..32]) {
        return Err(io::Error::other(
            "The WSL task does not belong to this home.",
        ));
    }
    let action = |keeper: &Path| {
        format!(
            r#"& "$env:SystemRoot\System32\wsl.exe" -d {} --cd ~ --exec /usr/bin/setsid --wait /usr/bin/env {} /bin/sh {} wsl-keep
exit $LASTEXITCODE"#,
            literal(&distro),
            literal(&format!("HOME={}", profile.display())),
            literal(&keeper.to_string_lossy())
        )
    };
    let old = format!(
        "{FLAGS} {}",
        encode(&action(&home.bin().join("kuvo-start")))
    );
    let new = format!(
        "{FLAGS} {}",
        encode(&action(&home.bin().join(".kuvo-start-keeper")))
    );
    windows(&powershell, &format!(r#"
$name={}
$task=Get-ScheduledTask -TaskName $name -TaskPath '\'
$program=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$user=$task.Principal.UserId
if ($user -notlike 'S-*') {{ $user=([Security.Principal.NTAccount]::new($user)).Translate([Security.Principal.SecurityIdentifier]).Value }}
if ($user -ne {} -or $task.Actions.Count -ne 1 -or $task.Actions[0].Execute -ne $program) {{ throw 'Not this installation''s task.' }}
if ($task.Actions[0].Arguments -ceq {}) {{ exit 0 }}
if ($task.Actions[0].Arguments -cne {}) {{ throw 'The WSL task has an unknown launcher.' }}
$action=New-ScheduledTaskAction -Execute $program -Argument {}
Set-ScheduledTask -TaskName $name -TaskPath '\' -Action $action | Out-Null
"#,literal(task),literal(sid),literal(&new),literal(&old),literal(&new))).await?;
    Ok(())
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn encode(value: &str) -> String {
    STANDARD.encode(
        value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
}

async fn windows(program: &Path, script: &str) -> io::Result<String> {
    let encoded = encode(&format!("$ErrorActionPreference = 'Stop';\n{script}"));
    output(
        program,
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
            &encoded,
        ],
    )
    .await
}

async fn output(program: &Path, args: &[&str]) -> io::Result<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "The WSL startup task did not answer.",
            )
        })??;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "The WSL startup task could not be updated: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}
