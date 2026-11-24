use super::{Computer, Destination, Existing, Job, Ran, Target, read_binary, run};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Duration};

impl Destination {
    pub(super) async fn native(mut self, job: &Job, host: &str) -> Result<Self, String> {
        let probe = r#"
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::WriteLine($env:PROCESSOR_ARCHITECTURE)
if ((Test-Path -LiteralPath (Join-Path $K 'run/setup-card.json')) -or
    (Test-Path -LiteralPath (Join-Path $K 'run/link.json')) -or
    (Test-Path -LiteralPath (Join-Path $K 'bin/kuvo-link.js'))) {
    [Console]::WriteLine('installed')
    foreach ($path in @('machine-id', 'run/setup-card.json')) {
        $item = Get-Item -LiteralPath (Join-Path $K $path) -Force -ErrorAction SilentlyContinue
        $limit = if ($path -eq 'machine-id') { 34 } else { 200000 }
        if ($item -and !$item.PSIsContainer -and $item.Length -le $limit -and !($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            $tag = if ($path -eq 'machine-id') { 'kuvo id ' } else { 'card ' }
            [Console]::WriteLine($tag + (Get-Content -Raw -LiteralPath $item.FullName))
        }
    }
}
"#;
        let ran = self.powershell(job, probe, Vec::new(), 30).await?;
        let text = String::from_utf8_lossy(&ran.stdout);
        let mut lines = text.lines();
        let cpu = match lines.next().map(str::trim) {
            Some("AMD64") => "x86_64",
            Some("ARM64") => "aarch64",
            _ => {
                return Err(format!(
                    "Kuvo Code couldn’t check which Windows release {host} needs."
                ));
            }
        };
        if !ran.status.success() {
            return Err(format!(
                "Kuvo Code on {host} already belongs to another setup. To add this computer, connect over SSH to your own OS account."
            ));
        }
        self.target = Target { os: "windows", cpu };
        self.existing = Existing::read(&lines.collect::<Vec<_>>());
        Ok(self)
    }

    pub(super) async fn install_native(&self, job: &Job, path: PathBuf) -> Result<(), String> {
        let starter = ["kuvo-starter.exe", "Kuvo.exe"]
            .into_iter()
            .map(|name| path.with_file_name(name))
            .find(|path| path.is_file())
            .ok_or("The Windows release is missing Kuvo's sign-in starter.")?;
        self.copy_native(job, starter, "Kuvo.exe").await?;
        let unwind = path.with_file_name("libunwind.dll");
        if unwind.is_file() {
            self.copy_native(job, unwind, "libunwind.dll").await?;
        }
        self.copy_native(job, path, "kuvo-box.exe").await
    }

    async fn copy_native(&self, job: &Job, path: PathBuf, name: &str) -> Result<(), String> {
        let release = path.clone();
        let digest = tokio::task::spawn_blocking(move || {
            read_binary(&release).map(|bytes| hex::encode(Sha256::digest(bytes)))
        })
        .await
        .ok()
        .flatten()
        .ok_or("Couldn’t read the Kuvo Code release.")?;
        let stage = format!(
            ".ssh-box-{}",
            hex::encode(crate::random::<16>().map_err(|_| "Couldn’t prepare the copy.")?)
        );
        const CHECK: &str = r#"
foreach ($path in @($K, (Join-Path $K 'bin'), (Join-Path $K 'run'))) {
    if (Test-Path -LiteralPath $path) {
        $item = Get-Item -LiteralPath $path -Force
        if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { exit 78 }
    }
}
if ((Test-Path -LiteralPath (Join-Path $K 'run/setup-card.json')) -or
    (Test-Path -LiteralPath (Join-Path $K 'run/link.json')) -or
    (Test-Path -LiteralPath (Join-Path $K 'bin/kuvo-link.js'))) { exit 78 }
"#;
        let acl = include_str!("windows-home.ps1");
        let prepare =
            format!("{CHECK}\n{acl}\n[void][IO.Directory]::CreateDirectory((Join-Path $K 'bin'))");
        let failed = || format!("Kuvo Code’s files couldn’t be put on {}.", self.name);
        if !self
            .powershell(job, &prepare, Vec::new(), 30)
            .await?
            .status
            .success()
        {
            return Err(failed());
        }
        // Native sshd can stall while streaming bytes to a PowerShell console.
        // SFTP uses the same trusted host and does not send the installer ticket.
        let ssh = self.ssh.clone();
        let stop = job.stop.clone();
        let remote = format!("{}/bin/{stage}", self.root);
        let copied = tokio::task::spawn_blocking(move || {
            ssh.copy(&path, &remote, Duration::from_secs(180), &stop)
        })
        .await
        .is_ok_and(|result| result.is_ok_and(|result| result.status.success()));
        let placed = if copied {
            let publish = format!(
                r#"
{CHECK}
$bin = Join-Path $K 'bin'
$temp = Join-Path $bin '{stage}'
$item = Get-Item -LiteralPath $temp -Force
if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {{ exit 78 }}
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash.ToLowerInvariant() -ne '{digest}') {{ exit 1 }}
$target = Join-Path $bin '{name}'
if (Test-Path -LiteralPath $target) {{
    $existing = Get-Item -LiteralPath $target -Force
    if ($existing.PSIsContainer -or ($existing.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        (Get-FileHash -Algorithm SHA256 -LiteralPath $target).Hash.ToLowerInvariant() -ne '{digest}') {{ exit 78 }}
    [IO.File]::Delete($temp)
}} else {{
    [IO.File]::Move($temp, $target)
}}
"#
            );
            self.powershell(job, &publish, Vec::new(), 30)
                .await
                .is_ok_and(|result| result.status.success())
        } else {
            false
        };
        if !placed {
            let clean = format!(
                r#"
{CHECK}
$temp = Join-Path $K 'bin/{stage}'
if (Test-Path -LiteralPath $temp) {{ [IO.File]::Delete($temp) }}
"#
            );
            let command = self.powershell_command(&clean);
            let ssh = self.ssh.clone();
            let _ = tokio::task::spawn_blocking(move || {
                ssh.run(
                    &command,
                    Vec::new(),
                    Duration::from_secs(10),
                    &AtomicBool::new(false),
                )
            })
            .await;
            return Err(failed());
        }
        Ok(())
    }

    pub(super) async fn enroll_native(
        &self,
        job: &Job,
        name: &str,
        code: &str,
    ) -> Result<(Computer, String), String> {
        let quote = name.replace('\'', "''");
        let script = format!(
            r#"
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Console]::OutputEncoding
$env:KUVO_HOME = $K
$join = [Console]::ReadLine()
$box = Join-Path $K 'bin/kuvo-box.exe'
$enrolled = $join | & $box ssh-enroll '{quote}'
if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}
& $box install --startup | Out-Null
if ($LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}
$enrolled
exit 0
"#
        );
        let mut input = code.as_bytes().to_vec();
        input.push(b'\n');
        let ran = self.powershell(job, &script, input, 90).await?;
        super::enrollment(ran, name)
    }

    async fn powershell(
        &self,
        job: &Job,
        script: &str,
        input: Vec<u8>,
        seconds: u64,
    ) -> Result<Ran, String> {
        run(
            job,
            &self.ssh,
            self.powershell_command(script),
            input,
            seconds,
        )
        .await
    }

    fn powershell_command(&self, script: &str) -> String {
        let script = format!(
            "$ErrorActionPreference='Stop'\n$K=Join-Path $env:USERPROFILE '{}'\n{script}",
            self.root,
        );
        let body = STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        format!("powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {body}")
    }
}
