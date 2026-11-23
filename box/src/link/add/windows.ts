// Commands understood by both cmd.exe and PowerShell under Windows OpenSSH.
import { distroName, windowsHostName } from "../../shared/wsl";

export const WINDOWS_PROBE = 'cmd.exe /d /c echo KUVO-WINDOWS-%COMPUTERNAME%';
export const WSL_LIST = "wsl.exe --list --verbose";

export function windowsHost(answer: string): string | undefined {
  return windowsHostName(/^KUVO-WINDOWS-([A-Za-z0-9-]+)\s*$/m.exec(answer)?.[1]);
}

export function selectDistro(answer: string, saved?: string): string | undefined {
  const distros = answer.replace(/[\0\uFEFF]/g, "").split("\n").flatMap(line => {
    const name = distroName(/^\s*\*?\s*([\w.-]+)\s{2,}.+?\s{2,}2\s*$/.exec(line)?.[1]);
    // These are WSL internals rather than user distributions.
    return name && !/^docker-desktop(?:-data)?$/i.test(name) ? [name] : [];
  });
  if (saved) return distros.find(name => name === saved);
  return distros.find(name => /^ubuntu(?:-|$)/i.test(name)) ?? distros[0];
}

// A cmd.exe command line is too short for the install scripts. Send one bounded ASCII
// line first. Read it unbuffered so tar, the engine archive and credentials stay on stdin.
const BOOTSTRAP = "import base64,os,sys; script=base64.b64decode(sys.stdin.buffer.raw.readline(262144)); os.execv('/bin/sh',['sh','-c','kuvo_script=1; '+script.decode()])";
export function wslBootstrap(distro: string): string {
  return `wsl.exe -d ${distro} --cd ~ --exec /usr/bin/python3 -c "${BOOTSTRAP}"`;
}

export const scriptHeader = (script: string) => Buffer.from(Buffer.from(script).toString("base64") + "\n");
