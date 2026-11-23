# Released Mac conformance fixtures

Build the TypeScript reference with `scripts/build-built.sh` and use Node 24.
Run the selected areas with the binary you want to check.

```sh
KUVO_BOX_BIN=box/dist/kuvo-link.js KUVO_CONFORMANCE_AREA=compat node --test protocol/conformance
KUVO_BOX_BIN=box/dist/kuvo-link.js KUVO_CONFORMANCE_AREA=holder node --test protocol/conformance
node protocol/conformance/compat/generate.mjs
node protocol/conformance/compat/reproduce.mjs
```

The generator prints the path of a new cache entry. Pass an existing scratch directory
as its first argument to create the entry there. For runs selecting Rust, set
`KUVO_CONFORMANCE_TS_BIN` if the reference bundle lives outside `box/dist`. The cache
contains two tagged machine homes, two tagged Mac app folders on macOS, a TypeScript beta
home and the `hold/1` transcript. Keep the cache at its generated path because the app
folders contain absolute paths in their Git records and engine databases. The generated
files stay out of Git.

Each historical generator opens a detached worktree at the release tag. The tagged link
bundle writes the machine records and answers the tagged setup and SSH settings inputs.
The macOS generator uses the release's Swift Codable types and checks the mail bytes with
the release's Swift codecs. It uses the existing OpenCode 2.0.32 database and the engine and
Keychain stand-ins for the staged move. Linux runs both machine scenarios. The two app
moves require APFS and `plutil` and run on macOS.

The current-beta home covers layout 23, link identity and signed setup checkpoints, projects
and Git settings, engine settings and a synthetic integration key, personalities, time zone,
Work Queue items and numbering, a paused conversation queue, agent and group transcripts,
letters and move records, usage settings and Insights, a wrapped vault and a sealed backup
archive. Remaining families from the [stored-format inventory](../../../docs/architecture/stored-formats.md)
need fixtures before this covers the full current-beta home.

The byte comparison substitutes cache roots and the OpenCode snapshot IDs derived from
those paths. It compares SQLite schema and committed rows through the WAL. Git stat
indexes, SQLite WAL and shared-memory file bytes, the beta.1 last-heard clock and holder
and agent PIDs vary. The tagged commits, payload bytes, SSH requests and responses and
other file contents must match.

The holder transcript records both directions through the real holder process. It includes
a fenced controller, replay of a pending request, a reply to the same running agent, stderr,
a signal and orderly shutdown. A Rust resident adopting a TypeScript holder end to end is
part of phase 4.1.

Generate the ignored `protocol/vectors/wire.json` with
`UPDATE_WIRE_VECTORS=1 scripts/test-box.sh box test/wire-vectors.test.ts`. The Rust wire test
consumes the six signed document kinds, their exchange hashes and the canonical JSON
digests from that file.
