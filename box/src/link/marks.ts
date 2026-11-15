// What starts this box from outside its folder, kept apart from the uninstaller that uses them so a
// module that writes those starts can be loaded before it, and first.

/** The launch agent that starts the box at login on a Mac (RemoteEngine.swift's `launchAgent`; the
 * helper's after #333 piece 4): one per computer, whatever set it up. */
export const LAUNCH_AGENT = "run.kuvo.code.machine";
/** The mark on the crontab line that starts the box after a restart (RemoteEngine.swift's `rebootTag`). */
export const REBOOT_TAG = "# kuvo-code: starts its engine again after a restart";
