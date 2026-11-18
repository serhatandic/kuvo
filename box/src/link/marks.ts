// The box's start marks, kept out of `uninstall.ts`: its imports reach the modules that write these
// starts, and one that read them from it as it loaded found them unset when it came first.

/** The launch agent that starts the box at login on a Mac (RemoteEngine.swift's `launchAgent`; the
 * helper's after #333 piece 4): one in each account’s launchd GUI domain. */
export const LAUNCH_AGENT = "run.kuvo.code.machine";
/** The mark on the crontab line that starts the box after a restart (RemoteEngine.swift's `rebootTag`). */
export const REBOOT_TAG = "# kuvo-code: starts its engine again after a restart";

/** SSH setup found a different setup already on this account. */
export const SETUP_OCCUPIED = 4;

export const ADD_RESERVATION = ".kuvo-code-adding";
