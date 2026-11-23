// Shared OpenSSH stand-in for adding a computer.
export const addSshStandIn = `#!/bin/sh
F="$FAKE_SSH"
strict=; known=; pubkey=yes; control=; alias=; config=
while [ $# -gt 0 ]; do
  case "$1" in
    -G) config=1; shift ;;
    -o) echo "$2" >>"$F/options"; case "$2" in
          HostKeyAlias=*) alias=\${2#*=} ;;
          StrictHostKeyChecking=*) strict=\${2#*=} ;;
          UserKnownHostsFile=*) known=\${2#*=} ;;
          PubkeyAuthentication=*) pubkey=\${2#*=} ;;
        esac; shift 2 ;;
    -O) control=$2; shift 2 ;;
    --) shift; break ;;
    *) shift ;;
  esac
done
dest=$1; cmd=$2
[ -n "$config" ] && { printf 'hostname %s\\nport 22\\n' "\${dest#*@}"; exit 0; }
echo "$dest $strict \${control:-run}" >>"$F/log"
[ -n "$control" ] && exit 0
case "$dest" in nowhere) echo "ssh: Could not resolve hostname nowhere: Name or service not known" >&2; exit 255 ;; esac
[ -f "$F/away-$dest" ] && { echo "ssh: connect to host $dest port 22: No route to host" >&2; exit 255; }
# (Reached at another of its addresses: its key is checked as the one known for the first's.)
[ -n "$alias" ] && dest=$alias
key=$(cat "$F/hostkey")
eval "set -- $known"
if [ "$strict" = accept-new ]; then
  echo "$dest $key" >>"$1"
  echo "Warning: Permanently added '$dest' (ED25519) to the list of known hosts." >&2
else
  found=; other=
  for file in "$@"; do
    [ -f "$file" ] || continue
    if grep -qxF "$dest $key" "$file"; then found=1; elif grep -q "^$dest " "$file"; then other=$file; fi
  done
  if [ -z "$found" ] && [ -n "$other" ]; then
    printf '%s\\n' "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@" "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @" "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@" "Offending ED25519 key in $other:1" "Host key for $dest has changed and you have requested strict checking." "Host key verification failed." >&2
    exit 255
  fi
  if [ -z "$found" ]; then
    printf '%s\\n' "No ED25519 host key is known for $dest and you have requested strict checking." "Host key verification failed." >&2
    exit 255
  fi
fi
if [ "$pubkey" = no ] || [ -f "$F/refuse" ]; then echo "you@$dest: Permission denied (publickey)." >&2; exit 255; fi
if [ -f "$F/windows-$dest" ]; then
  case "$cmd" in
    cmd.exe*) printf 'KUVO-WINDOWS-DESKTOP-TEST\\r\\n'; exit 0 ;;
    'wsl.exe --list --verbose') printf '  NAME  STATE  VERSION\\r\\n* Ubuntu  Stopped  1\\r\\n' | iconv -f UTF-8 -t UTF-16LE; exit 0 ;;
    *) echo "'exec' is not recognized as an internal or external command." >&2; exit 255 ;;
  esac
fi
[ -f "$F/home-$dest" ] && FAKE_SSH_HOME=$(cat "$F/home-$dest")
# A detached simulated SSH command stays identifiable by its script even when macOS
# hides the environment of /bin/sh and its SSH parent has gone away.
remote_script=$(mktemp "$F/ssh-command.XXXXXXXX") || exit 1
(umask 077; printf '%s\\n' "$cmd" >"$remote_script") || exit 1
if [ -f "$F/trace" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in
    *'uname -sm'*) action=probe ;;
    *'kuvo-link.js" box seed'*) action=seed ;;
    *'kuvo-link.js" setup'*) action=join ;;
    *'kuvo-link.js" uninstall'*) action=uninstall ;;
    *'then rm -f "$R"; fi'*|*'if [ "$prior" = '*'rm -f "$A"'*) action=release ;;
    *) action=other ;;
  esac
  echo "$dest $action" >>"$F/trace"
  if [ "$action" = release ] && [ -f "$F/fail-release-$dest" ]; then
    echo "rm: test reservation could not be cleared" >&2
    exit 1
  fi
  if [ "$action" = seed ] && [ -f "$F/drop-seed-$dest" ]; then
    cat >"$F/input-$dest"
    (
      HOME="$FAKE_SSH_HOME" sh "$remote_script" <"$F/input-$dest"
      : >"$FAKE_SSH_HOME/writer-ended"
    ) >/dev/null 2>&1 &
    while [ ! -f "$FAKE_SSH_HOME/writing" ] && [ ! -f "$FAKE_SSH_HOME/writer-ended" ]; do sleep 0.05; done
    if [ "$(cat "$F/drop-seed-$dest")" = 255 ]; then
      echo "Connection to $dest closed by remote host." >&2
      exit 255
    fi
    kill -TERM "$$"
  fi
fi
# Exercise a remote bridge that still returns the revoked credentials on SSH Add.
if [ -f "$F/stale-join-$dest" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *'kuvo-link.js" setup'*) cat >/dev/null; cat "$F/stale-join-$dest"; exit 0 ;; esac
fi
if [ -f "$F/track" ] || [ -f "$F/lose-uninstall-reply" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  if [ -f "$F/track" ]; then
    case "$text" in *'kuvo kind'*) echo probe >>"$F/track" ;; *'bin/kuvo-start" start'*) echo start >>"$F/track" ;; *'kuvo-link.js" box seed'*) echo accounts >>"$F/track" ;; esac
  fi
  if [ -f "$F/lose-uninstall-reply" ]; then
    case "$text" in *'kuvo-link.js" uninstall'*)
      if [ "$(cat "$F/lose-uninstall-reply")" = before ]; then
        : >"$F/uninstall-entered"
        while [ ! -f "$F/uninstall-continue" ]; do sleep 0.05; done
        echo "the connection closed before the command ran" >&2
        exit 255
      fi
      HOME="$FAKE_SSH_HOME" sh "$remote_script" >/dev/null 2>&1
      echo "lost the reply after cleaning $FAKE_SSH_HOME/.kuvo-code" >&2
      exit 255 ;;
    esac
  fi
fi
if [ -f "$F/fail" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *"$(cat "$F/fail")"*) cat >/dev/null; echo "it broke" >&2; exit 1 ;; esac
fi
hold="$F/hold-$dest"; [ -f "$hold" ] || hold="$F/hold"
if [ -f "$hold" ]; then
  text=$(printf %s "$cmd" | sed -n 's/.*printf %s \\([A-Za-z0-9+/=]*\\) | base64 -d.*/\\1/p' | base64 -d 2>/dev/null)
  case "$text" in *"$(cat "$hold")"*) while [ -f "$hold" ]; do sleep 0.05; done ;; esac
fi
HOME="$FAKE_SSH_HOME" exec sh "$remote_script"
`;
