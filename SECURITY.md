# Security policy

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Use GitHub's
[private vulnerability reporting](https://github.com/serhatandic/kuvo/security/advisories/new)
so the report, reproduction steps, and any proposed fix remain private.

Include the affected version and macOS version, the impact, and the smallest
reproduction you can provide. Kuvo's logs can contain container names, image
names, and paths from your Mac, so remove anything identifying from logs and
screenshots before you share them.

You should receive an acknowledgement within seven days. Confirmed issues will
be coordinated privately until a fix and release are available.

## Scope

Reports are especially welcome about:

- the boundary between the Linux VM and the Mac, including the shared `/Users`
  folder and the Docker socket at `~/.kuvo/docker.sock`;
- ports forwarded to `localhost`;
- the downloads and checksums Kuvo uses to set up its VM;
- the `kuvo` command and the PATH changes Kuvo makes.

Vulnerabilities in Docker Engine or Alpine Linux themselves should go to those
projects. Tell us too if Kuvo's setup makes one easier to exploit.

## Supported versions

Kuvo is pre-1.0. Security fixes are made on `main` and released in the newest
build; older builds are not maintained separately.
