"""Stream a worktree and generated test fixtures to the Windows SSH runner."""

import base64
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import uuid


def git(root, *args):
    return subprocess.check_output(["git", "--no-optional-locks", "-C", root, *args])


def settings(root, kind, args):
    pattern = None
    if kind == "bridge" and args and args[0].startswith("--test-name-pattern="):
        pattern = args.pop(0)
    folder, extension = ("test", "ts") if kind == "box" else ("bridge-tests", "js")
    files = []
    for given in args:
        file = given.removeprefix("box/").removeprefix(folder + "/")
        if "/" not in file and kind == "box" and not (root / "box" / folder / file).is_file():
            file = "agents/" + file
        if not re.fullmatch(r"(?:[a-zA-Z0-9_][a-zA-Z0-9_.-]*/)*[a-zA-Z0-9_][a-zA-Z0-9_.-]*\.test\." + extension, file):
            raise ValueError(f"Invalid test file: {given}")
        if not (root / "box" / folder / file).is_file():
            raise ValueError(f"No {kind} test: {given}")
        files.append(f"{folder}/{file}")
    return {"kind": kind, "files": list(dict.fromkeys(files)), "pattern": pattern}


def history(root, selected):
    """Give tests the same pinned builds lokum stages for a checkout without .git."""
    link = "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js"
    bridge = root / "box/bridge-tests"
    fixtures = [
        ("devices-two-boxes failure-bench-legacy", "failure-bench-legacy-ref.txt", "failure-bench-legacy.mjs", link),
        ("folder-version updates", "4330569c2f1c39eef166f65d2abb1b7256ef7dcb", "layout-4.mjs", link),
        ("update-real-keeper", "11323b39316e0ec1f9b6281ab9bba9153db8e822", "layout-0.mjs", link),
        ("setup-handover", "recovery-neighbor-ref.txt", "recovery-neighbor.mjs", link),
        ("setup-handover", "fccda2de1", "beta35-owner.mjs", link),
        ("setup-handover resident-update-pilot", "split-launch-ref.txt", "split-launch.mjs", link),
        ("setup-handover resident-update-pilot", "split-launch-ref.txt", "split-keeper.sh", "box/src/keeper/kuvo-start"),
    ]
    for tests, ref, name, source in fixtures:
        if selected and not any(f"bridge-tests/{test}.test.js" in selected for test in tests.split()):
            continue
        if ref.endswith(".txt"):
            ref = (bridge / ref).read_text().strip()
        if not re.fullmatch(r"[a-f0-9]{8,40}", ref):
            raise ValueError(f"Invalid revision for {name}")
        yield f"box/bridge-tests/.{name}", git(root, "show", f"{ref}:{source}")


def snapshot(output, root, runner, config, wsl):
    # Match the Linux runners' test inputs, including sibling sources and built resources.
    scopes = ("box", "catalog", "protocol", "scripts", "swift", "mac/helper/Sources",
              "docs/architecture/stored-formats.md")
    tracked = {Path(os.fsdecode(name)) for name in git(root, "ls-files", "-z", "--", *scopes).split(b"\0") if name}
    with tarfile.open(fileobj=output, mode="w|gz", format=tarfile.PAX_FORMAT, dereference=not wsl) as tar:
        for name in sorted(tracked):
            file = root / name
            if set(name.parts) & {".git", "node_modules"}:
                continue
            if not (file.is_file() or file.is_symlink()):
                continue
            target = file.resolve()
            if not target.is_relative_to(root) or target.relative_to(root) not in tracked or not target.is_file():
                raise ValueError(f"Refusing to stage {name}: it resolves outside tracked regular files")
            if file.parent.resolve() != file.parent:
                raise ValueError(f"Refusing linked parent of {name}")
            tar.add(file, arcname=name.as_posix(), recursive=False)
        platform_scripts = ("wsl.sh",) if wsl else ("native.ps1", "job.ps1")
        for name in ("run.mjs", "pins.json", *platform_scripts):
            tar.add(runner / name, arcname=f".windows-checks/{name}", recursive=False)
        if config["kind"] == "bridge":
            for name, data in history(root, config["files"]):
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        tar.addfile(tarfile.TarInfo(".windows-checks/ready"))


def native_run(ssh, sftp, root, runner, config, item, payload):
    stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    name = f"{item}-{stamp}-{uuid.uuid4().hex[:8]}"
    bootstrap = (runner / "bootstrap.ps1").read_text()

    def remote(action):
        settings = f"$action='{action}'; $name='{name}'; $config='{payload}'\n"
        encoded = base64.b64encode((settings + bootstrap).encode("utf-16le")).decode()
        command = "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand " + encoded
        if len(command) > 8000:
            raise ValueError("The SSH command exceeds cmd.exe's length limit. Select fewer test files.")
        return subprocess.run([*ssh, command], stdin=subprocess.DEVNULL).returncode

    with tempfile.TemporaryDirectory(prefix=".windows-check-", dir=root) as scratch:
        archive = Path(scratch) / "repo.tar.gz"
        with archive.open("wb") as output:
            snapshot(output, root, runner, config, wsl=False)
        status = remote("create")
        if status:
            return status
        # Pass SFTP a relative local path to avoid quoting the worktree path in its batch language.
        try:
            status = subprocess.run(sftp, cwd=scratch, text=True,
                                    input=f"put repo.tar.gz kuvo-checks/{name}/repo.tar.gz\n").returncode
        except (KeyboardInterrupt, OSError):
            remote("discard")
            raise
        if status:
            remote("discard")
            return status
    return remote("run")


def wsl_run(ssh, root, runner, config, item, payload):
    bootstrap = base64.b64encode((runner / "bootstrap.sh").read_bytes()).decode()
    command = f"""wsl.exe -d Ubuntu-24.04 --exec /bin/bash -c "source <(base64 -d <<<'{bootstrap}')" -- {item} {payload}"""
    if len(command) > 8000:
        raise ValueError("The SSH command exceeds cmd.exe's length limit. Select fewer test files.")
    with subprocess.Popen([*ssh, command], stdin=subprocess.PIPE) as remote:
        transfer_error = None
        try:
            snapshot(remote.stdin, root, runner, config, wsl=True)
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            transfer_error = error
        finally:
            try:
                remote.stdin.close()
            except BrokenPipeError as error:
                transfer_error = transfer_error or error
        status = remote.wait()
    if transfer_error is not None:
        print(f"Checkout transfer: {transfer_error}", file=sys.stderr)
    return status or int(transfer_error is not None)


def main(kind, worktree, item, *args):
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,40}", item):
        raise ValueError("Item must contain 1–40 letters, digits, underscores or hyphens")
    wsl = bool(args and args[0] == "--wsl")
    root = Path(worktree).resolve(strict=True)
    config = settings(root, kind, list(args[1:] if wsl else args))
    runner = Path(__file__).resolve().parent
    payload = base64.b64encode(json.dumps(config).encode()).decode()
    host = os.environ.get("KUVO_WIN_HOST") or "100.101.128.69"
    if "@" not in host:
        host = "kuvo@" + host
    key = Path(os.environ.get("KUVO_WIN_KEY") or "~/.ssh/id_ed25519_wintest").expanduser()
    transport = ["-o", "BatchMode=yes", "-i", key, "--", host]
    ssh = ["ssh", "-T", *transport]
    if wsl:
        status = wsl_run(ssh, root, runner, config, item, payload)
    else:
        status = native_run(ssh, ["sftp", "-b", "-", *transport], root, runner, config, item, payload)
    return 128 - status if status < 0 else status


if __name__ == "__main__":
    try:
        sys.exit(main(*sys.argv[1:]))
    except (TypeError, ValueError, OSError) as error:
        print(f"{error}\nUsage: <check script> <worktree> <item> [--wsl] [test files…]", file=sys.stderr)
        sys.exit(64)
