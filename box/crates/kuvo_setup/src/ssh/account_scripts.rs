//! POSIX account lock commands shared by SSH setup operations.

use std::io;

pub(in crate::ssh) const BUSY: i32 = 8;
pub(in crate::ssh) const LOST: i32 = 73;
const BAD_ACCOUNT: i32 = 74;

pub(in crate::ssh) fn new_token() -> io::Result<String> {
    Ok(format!(
        "guarded-{}",
        hex::encode(crate::random::<24>().map_err(io::Error::other)?)
    ))
}

pub(in crate::ssh) fn claim_script(token: &str, preflight: &str) -> String {
    locked_script(
        0,
        &format!(
            r#"{preflight}
if claimed; then exit {BUSY}; fi
ln -s '{token}' "$A" || exit {BAD_ACCOUNT}"#
        ),
    )
}

pub(in crate::ssh) fn release_script(token: &str) -> String {
    locked_script(
        5,
        &format!(
            r#"
if claimed; then
  prior=$(readlink "$A") || exit {BAD_ACCOUNT}
  if [ "$prior" = '{token}' ]; then rm -f "$A"; fi
fi"#
        ),
    )
}

pub(in crate::ssh) fn locked_script(wait: u8, script: &str) -> String {
    format!(
        r#"A="$HOME/.kuvo-code-adding"
claimed() {{ [ -e "$A" ] || [ -L "$A" ]; }}
if [ -L "$A.lock" ] || {{ [ -e "$A.lock" ] && [ ! -f "$A.lock" ]; }}; then
  echo 'The Kuvo Code account lock must be a regular file.' >&2
  exit {BAD_ACCOUNT}
fi
(umask 077; : >>"$A.lock") || exit {BAD_ACCOUNT}
(
  case "$(uname -s)" in
    Darwin) /usr/bin/lockf -t {wait} 9 ;;
    *) flock -w {wait} -E 75 9 ;;
  esac
  status=$?
  if [ "$status" -ne 0 ]; then
    if [ "$status" -eq 75 ] && claimed; then exit {BUSY}; fi
    exit {BAD_ACCOUNT}
  fi
  trap '' HUP
  {script}
) 9<>"$A.lock""#
    )
}

pub(in crate::ssh) fn owns(token: &str) -> String {
    format!("[ \"$(readlink \"$A\" 2>/dev/null)\" = '{token}' ]")
}
