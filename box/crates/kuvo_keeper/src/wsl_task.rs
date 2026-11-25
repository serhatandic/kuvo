//! WSL 2's start at Windows sign-in: one hidden scheduled task per installation, named for the
//! Windows account, the distribution and the home, which runs the saved keeper's `wsl-keep`.
//! Cron doesn't run while no WSL session does, so this replaces its `@reboot` line there.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use kuvo_os::platform::{Account, Platform, is_wsl_windows_drive};
use kuvo_store::{files, home::Home};
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const FLAGS: [&str; 6] = [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-WindowStyle",
    "Hidden",
    "-EncodedCommand",
];
const RECORD: &str = "wsl-task";

/// This installation's task, once WSL 2 and Windows have said who and where it runs.
pub struct WslTask {
    powershell: PathBuf,
    sid: String,
    distro: String,
    profile: PathBuf,
    name: String,
}

impl WslTask {
    /// None outside WSL 2, or for a scratch home a test didn't ask a real task for.
    pub async fn for_home(home: &Home) -> io::Result<Option<Self>> {
        if Platform::current() != Platform::Wsl {
            return Ok(None);
        }
        let env = std::env::vars_os().collect();
        let account = Account::current()?;
        let profile = account.home_folder(Platform::Wsl, &env)?.canonicalize()?;
        let scratch = profile != account.home.canonicalize()?;
        if home.root().canonicalize()? != profile.join(".kuvo-code")
            || (scratch && std::env::var_os("KUVO_TEST_WSL_TASK").is_none())
        {
            return Ok(None);
        }
        let distro = std::env::var("WSL_DISTRO_NAME")
            .ok()
            .filter(|name| distro_name(name))
            .ok_or_else(|| io::Error::other("The WSL distribution is unknown."))?;
        let powershell = PathBuf::from(
            output(
                Path::new("/usr/bin/wslpath"),
                &[
                    "-u",
                    r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                ],
            )
            .await?
            .trim(),
        );
        let sid = windows(
            &powershell,
            "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
        )
        .await?
        .trim()
        .to_owned();
        if !sid
            .strip_prefix("S-")
            .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
        {
            return Err(io::Error::other(
                "The Windows account could not be identified.",
            ));
        }
        let root = home
            .root()
            .to_str()
            .ok_or_else(|| io::Error::other("The WSL home has an invalid path."))?;
        let identity = serde_json::to_vec(&[sid.as_str(), &distro, root])?;
        let name = format!(
            "KuvoCode-WSL-{}",
            &crate::manifest::sha256(&identity)[..32]
        );
        Ok(Some(Self {
            powershell,
            sid,
            distro,
            profile,
            name,
        }))
    }

    /// Registers the task (again) and starts it; its record says the keeper is the task's.
    pub async fn register(&self, home: &Home) -> io::Result<()> {
        self.put(home, true).await
    }

    async fn put(&self, home: &Home, start: bool) -> io::Result<()> {
        keep_linux_path(home)?;
        files::create_private_dirs(&home.run())?;
        files::write_secret(&home.run().join(RECORD), format!("{}\n", self.name).as_bytes())?;
        let action = self.action(&crate::shell_keeper(&home.bin()));
        windows(&self.powershell, &format!(r#"
$user = {user}
$name = {name}
$action = New-ScheduledTaskAction -Execute (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -Argument {action}
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -Hidden -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
# OpenSSH can register with an elevated token while the task runs as the same
# user with a limited token. Give that user SID control of this task directly.
$scheduler = New-Object -ComObject 'Schedule.Service'
$scheduler.Connect()
$task = $scheduler.GetFolder('\').GetTask($name)
$acl = [Security.AccessControl.RawSecurityDescriptor]::new($task.GetSecurityDescriptor(4))
$ace = [Security.AccessControl.CommonAce]::new('None', 'AccessAllowed', 0x1f01ff, [Security.Principal.SecurityIdentifier]::new($user), $false, $null)
$acl.DiscretionaryAcl.InsertAce(0, $ace)
$task.SetSecurityDescriptor($acl.GetSddlForm('Access'), 0)
{start}"#,
            user = literal(&self.sid),
            name = literal(&self.name),
            action = literal(&action),
            start = if start { "Start-ScheduledTask -TaskName $name" } else { "" },
        ))
        .await
        .map(drop)
    }

    /// A task an earlier release registered starts the ordinary keeper, which a released Mac
    /// can replace: registered again for the saved one, without starting a second keeper.
    pub async fn repair(&self, home: &Home) -> io::Result<()> {
        let saved = crate::shell_keeper(&home.bin());
        if files::text_of(&home.run().join(RECORD)).as_deref().map(str::trim) != Some(&self.name) {
            return Ok(());
        }
        let current = windows(&self.powershell, &format!(
            "(Get-ScheduledTask -TaskName {} -TaskPath '\\').Actions[0].Arguments",
            literal(&self.name)
        ))
        .await?;
        if current.trim() == self.action(&saved) {
            return Ok(());
        }
        self.put(home, false).await
    }

    /// Removes only this installation's task, leaving its distribution and account alone. The
    /// keeper it started stops by the ordinary stop; the task gone, nothing starts it again.
    pub async fn remove(&self, home: &Home) -> io::Result<()> {
        if files::text_of(&home.run().join(RECORD)).as_deref().map(str::trim)
            != Some(self.name.as_str())
        {
            return Ok(());
        }
        windows(&self.powershell, &format!(r#"
$name = {name}
$task = Get-ScheduledTask -TaskName $name -TaskPath '\' -ErrorAction SilentlyContinue -ErrorVariable lookup
if ($task) {{
  $task | Unregister-ScheduledTask -Confirm:$false
}} elseif ($lookup | Where-Object {{ $_.CategoryInfo.Category -ne 'ObjectNotFound' }}) {{
  throw $lookup[0]
}}
# One already gone is gone: the lookup's error isn't the exit code.
exit 0"#,
            name = literal(&self.name),
        ))
        .await?;
        files::remove_file_synced(&home.run().join(RECORD))
    }

    fn action(&self, keeper: &Path) -> String {
        let script = format!(
            "& \"$env:SystemRoot\\System32\\wsl.exe\" -d {} --cd ~ --exec /usr/bin/setsid --wait /usr/bin/env {} /bin/sh {} wsl-keep\nexit $LASTEXITCODE",
            literal(&self.distro),
            literal(&format!("HOME={}", self.profile.display())),
            literal(&keeper.to_string_lossy()),
        );
        format!("{} {}", FLAGS.join(" "), encode(&script))
    }
}

/// The task's keeper starts from Windows: its saved PATH keeps only Linux folders.
fn keep_linux_path(home: &Home) -> io::Result<()> {
    let launch = home.run().join("launch.env");
    let Some(env) = files::text_of(&launch) else {
        return Ok(());
    };
    let linux: Vec<String> = env
        .lines()
        .map(|line| match line.strip_prefix("PATH=") {
            Some(list) => format!(
                "PATH={}",
                list.split(':')
                    .filter(|folder| !is_wsl_windows_drive(Path::new(folder)))
                    .collect::<Vec<_>>()
                    .join(":")
            ),
            None => line.to_owned(),
        })
        .collect();
    let linux = linux.join("\n") + if env.ends_with('\n') { "\n" } else { "" };
    if linux != env {
        files::write_secret(&launch, linux.as_bytes())?;
    }
    Ok(())
}

fn distro_name(name: &str) -> bool {
    name.len() <= 80
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
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
    let encoded = encode(&format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue';\n{script}"
    ));
    let mut args = FLAGS.to_vec();
    args.push(&encoded);
    output(program, &args).await
}

async fn output(program: &Path, args: &[&str]) -> io::Result<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Windows did not answer."))??;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "Windows couldn't change the sign-in task: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}

/// For uninstall: whether this installation has a task (checking), or removes it. None when
/// it can't be confirmed (another home, or Windows not answering).
pub fn uninstall(home: &Home, check: bool) -> io::Result<Option<bool>> {
    if !home.run().join(RECORD).exists() {
        return Ok(Some(false));
    }
    if check {
        return Ok(Some(true));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        match WslTask::for_home(home).await {
            Ok(Some(task)) => match task.remove(home).await {
                Ok(()) => Ok(Some(true)),
                Err(error) => {
                    eprintln!("Removing the Windows sign-in task: {error}");
                    Ok(None)
                }
            },
            Ok(None) => Ok(None),
            Err(error) => {
                eprintln!("Finding the Windows sign-in task: {error}");
                Ok(None)
            }
        }
    })
}
