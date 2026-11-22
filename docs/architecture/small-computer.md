# Kuvo on a small computer

The resident owns the box's local services. The keeper and coding-agents
bridge stay available with it. A session, a due wake-up or an engine-backed
operation can start OpenCode. The resident parks an idle engine after about ten
minutes and keeps it running while a turn, question, command or terminal needs
it. Explicit engine start and release health checks still exercise the engine.
Chrome starts when a browser tool or view needs it and closes after ten idle
minutes, even if an agent's MCP connection remains open.

Run `scripts/small-computer-check.sh` from the checkout after rebuilding the
bundles. It uses lokum-local by default and runs Debian 13 with `--cpus=2`,
`--memory=2g` and swap disabled. It mounts only a copy of the build and an
empty test home. The host's installed OpenCode binary is copied without its
settings, data or credentials. `HOST` and `ENGINE` can name a different Docker
host and binary. The check measures five minutes of idle, then completes one
turn and two concurrent turns on OpenCode Zen's free `opencode/big-pickle`.

The idle budget is 300 MiB of peak summed PSS and 5% of one CPU core averaged
over the five minutes. PSS divides shared pages among the processes mapping
them. RSS is recorded for comparison. The Docker cgroup CPU reading includes
short-lived upkeep jobs and the sampler itself. Per-process memory is taken
at the same instant as the corresponding total peak. The sampler and Docker
init are excluded from the memory total.

To collect numbers on a previous revision, run
`REVISION=543b69ae7 scripts/small-computer-check.sh --measure`. The same
scenario runs without enforcing the budget.

## Lokum measurements

Measured on November 22, 2026 with OpenCode 2.2.6. The baseline was
`543b69ae7`. The resident build was `770171271`, after merging local main.

| Scenario | Baseline peak PSS | Resident peak PSS | Baseline CPU | Resident CPU |
| --- | ---: | ---: | ---: | ---: |
| Idle, five minutes | 726.5 MiB | 266.9 MiB | 14.71% | 3.21% |
| One free-model turn | 647.5 MiB | 896.4 MiB | 39.13% | 143.21% |
| Two concurrent free-model turns | 672.2 MiB | 690.2 MiB | 61.57% | 59.07% |

The single-turn run includes the resident build's cold engine start. At that
peak, two temporary tool-metadata commands contributed about 241 MiB PSS.
The baseline already had a warm engine. Both concurrent-turn runs used a warm engine.

Per-process readings are RSS / PSS in MiB, taken at each scenario's total peak.

| Process | Baseline idle | Resident idle | Baseline two turns | Resident two turns |
| --- | ---: | ---: | ---: | ---: |
| Resident serve | 239.2 / 206.6 | 259.8 / 240.6 | 242.7 / 210.6 | 262.9 / 232.5 |
| Separate box services | 152.0 / 119.8 | — | 155.0 / 123.8 | — |
| Agents bridge | 47.4 / 21.6 | 43.6 / 25.0 | 48.1 / 21.7 | 48.3 / 23.2 |
| OpenCode | 312.0 / 279.5 | — | 335.6 / 303.7 | 451.2 / 421.2 |
| Browser installation | 115.4 / 86.7 | — | — | — |
| Browser MCP | 32.1 / 11.5 | — | 31.4 / 11.3 | 30.7 / 12.5 |
| Keeper and its sleep process | 3.6 / 0.8 | 3.7 / 1.3 | 3.8 / 1.1 | 3.6 / 0.8 |
