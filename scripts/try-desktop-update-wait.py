#!/usr/bin/env python3
"""Waits for what scripts/try-desktop-update.sh asks, and gives up after LIMIT seconds (exit 1):

    try-desktop-update-wait.py log FILE TEXT LIMIT          TEXT is in the log FILE
    try-desktop-update-wait.py version FOLDER VERSION LIMIT  FOLDER/Kuvo.app is VERSION
    try-desktop-update-wait.py exited PID LIMIT             the process PID has ended
    try-desktop-update-wait.py launched ID APP LIMIT        an app with bundle ID exactly ID runs
                                                            from APP: prints its PID

The first three wait on the system's word (kqueue: a file written, a folder changed, a process
ended). An app opened by someone else (Sparkle relaunching the update) gives no such word to
wait on, so `launched` asks Launch Services (lsappinfo) again each half second. Nothing is ever
signalled or killed here."""
import os
import plistlib
import re
import select
import subprocess
import sys
import time


def gave_up(limit, what):
    sys.exit(f"gave up after {limit:.0f}s waiting for {what}")


def on_change(path, done, limit, what):
    fd = os.open(path, os.O_RDONLY)
    queue = select.kqueue()
    event = select.kevent(fd, select.KQ_FILTER_VNODE, select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                          select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND | select.KQ_NOTE_RENAME | select.KQ_NOTE_LINK)
    end = time.monotonic() + limit
    while not done():
        left = end - time.monotonic()
        if left <= 0:
            gave_up(limit, what)
        queue.control([event], 1, min(left, 5))


def logged(path, text):
    try:
        with open(path, errors="replace") as log:
            return text in log.read()
    except OSError:
        return False


def version_is(folder, version):
    try:
        with open(os.path.join(folder, "Kuvo.app/Contents/Info.plist"), "rb") as info:
            return plistlib.load(info).get("CFBundleShortVersionString") == version
    except OSError:
        return False


def exited(pid, limit):
    queue = select.kqueue()
    try:
        queue.control([select.kevent(pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_ONESHOT, select.KQ_NOTE_EXIT)], 0, 0)
    except ProcessLookupError:
        return  # (Gone already.)
    if not queue.control(None, 1, limit):
        gave_up(limit, f"process {pid} to end")


def running(bundle_id, app):
    """The PID of the app Launch Services knows by exactly this bundle ID, running from APP."""
    found = subprocess.run(["lsappinfo", "find", f"bundleid={bundle_id}"], capture_output=True, text=True).stdout
    for asn in re.findall(r"ASN:0x[0-9a-fA-F]+-0x[0-9a-fA-F]+", found):
        said = subprocess.run(["lsappinfo", "info", asn], capture_output=True, text=True).stdout
        named = re.search(r'bundleID="([^"]*)"', said)
        path = re.search(r'bundle path="([^"]*)"', said)
        pid = re.search(r"\bpid\s*=\s*(\d+)", said)
        if named and named.group(1) == bundle_id and path and pid and os.path.realpath(path.group(1)) == os.path.realpath(app):
            return int(pid.group(1))
    return None


def launched(bundle_id, app, limit):
    end = time.monotonic() + limit
    while (pid := running(bundle_id, app)) is None:
        left = end - time.monotonic()
        if left <= 0:
            gave_up(limit, f"{bundle_id} to open from {app}")
        select.select([], [], [], min(left, 0.5))
    print(pid)


def main():
    kind, args = sys.argv[1], sys.argv[2:]
    if kind == "log":
        path, text, limit = args[0], args[1], float(args[2])
        on_change(path, lambda: logged(path, text), limit, text)
    elif kind == "version":
        folder, version, limit = args[0], args[1], float(args[2])
        on_change(folder, lambda: version_is(folder, version), limit, f"Kuvo {version}")
    elif kind == "exited":
        exited(int(args[0]), float(args[1]))
    elif kind == "launched":
        launched(args[0], args[1], float(args[2]))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
