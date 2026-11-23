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

The current-beta home has a representative for every row in the stored-format inventory.
The [family list](current-families.md) names its writer, path and reopening assertion.
The test checks that the list and inventory agree. It exercises the TypeScript writers
with synthetic input and uses loopback stand-ins for OpenCode and Cursor. Provider,
Chromium, desktop and retired app input is checked for preservation.

The reproduction check substitutes cache roots and the OpenCode snapshot, fence policy
and artifact IDs derived from those paths. It compares SQLite schema and committed rows
through the WAL. The artifact index's journal inode and mtime, Git stat indexes, SQLite
WAL and shared-memory bytes, bot instruction hashes that include scratch paths, account
and wake IDs, beta.1's last-heard clock, and recorded process IDs can vary. The tagged
commits, payload bytes, SSH requests and responses and other file contents must match.
Use `node protocol/conformance/compat/reproduce.mjs --current` to check the current home
alone.

The holder transcript records both directions through the real holder process. It includes
a fenced controller, replay of a pending request, a reply to the same running agent, stderr,
a signal and orderly shutdown. A Rust resident adopting a TypeScript holder end to end is
part of phase 4.1.

Generate the ignored `protocol/vectors/wire.json` with
`UPDATE_WIRE_VECTORS=1 scripts/test-box.sh box test/wire-vectors.test.ts`. The Rust wire test
consumes the six signed document kinds, their exchange hashes and the canonical JSON
digests from that file.
