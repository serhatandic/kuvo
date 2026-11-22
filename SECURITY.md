# Security policy

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Use GitHub's
[private vulnerability reporting](https://github.com/serhatandic/kuvo-code/security/advisories/new)
so the report, reproduction steps, and any proposed fix remain private. If you
can't use it, email [info@kuvo.run](mailto:info@kuvo.run) instead.

Include the affected desktop version and macOS version, and the affected box
version, platform and connection type (local, SSH or relay) as applicable.
Describe the impact and the smallest
reproduction you can provide. Kuvo Code's sessions and logs can contain your
prompts, code, file paths and account names, so remove anything identifying
from logs and screenshots before you share them.

You should receive an acknowledgement within seven days. Confirmed issues will
be coordinated privately until a fix and release are available.

## Scope

Reports are especially welcome about:

- signed releases, engine downloads and checks before activation
- box discovery, private local HTTP, paired SSH and encrypted relay access
- browser tools, the keeper socket, and what pages and clients can reach
- box-owned approvals, sandbox rules and agent permission delivery on macOS
  and Linux
- vault storage, sign-ins, API keys and usage credentials
- attachments and the context a box passes to a model

Vulnerabilities in opencode itself should go to the
[opencode project](https://github.com/anomalyco/opencode). Tell us too if Kuvo
Code's setup makes one easier to exploit.

Kuvo Code does not accept outside pull requests or code contributions. For
non-security bugs and ideas, use [GitHub issues](https://github.com/serhatandic/kuvo-code/issues).

## Supported versions

Kuvo Code is pre-1.0. Security fixes go to `main`. The first GPUI release is
still in [release verification](docs/releases.md). Older builds are not
maintained separately.
