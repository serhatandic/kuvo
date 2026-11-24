# Writing tests in Kuvo Code

Read this before writing a test. There are no AI review passes: the user tests by hand, and the
tests are the only automated guard. A test is worth having only if it fails when the feature breaks
for a user.

Where tests live:

- **Box** (Rust): process-level tests in `box/crates/<crate>/tests/`, using a scratch home.
- **Box** (Bun, TypeScript): `box/test/*.test.ts`. The first import is `./safety`.
- **Bridge and route scenarios** (Node's runner, the built bridge and link): `box/bridge-tests/*.test.js`,
  first import `./test-safety.js`; route scenarios in `box/test/fixtures/routes/*.json` (CONTRIBUTING.md,
  "Route scenarios").
- **Desktop** (Rust): `gpui/kuvo/tests/*.rs` (views, against the stand-in box `kuvo::stand_in`),
  `gpui/kuvo_box/tests/*.rs` (the client of a box).

## 1. Test what the user gets

Assert what the app shows, keeps, sends or does: a route's answer, the message delivered, the file
on disk, the text in the view. Not which function was called or what a private field holds.

For every test ask: **would this fail if the feature broke for a user?** If not, delete it.

Good (`box/test/conversation.test.ts`): what `read_conversation` gives of a long conversation, by
its text:

```ts
expect(read).toContain("[… the middle is left out; read it with query, to find a part …]")
expect(read).toContain("Paragraph 200 ")
```

Good (`gpui/kuvo/tests/send_while_working.rs`): keys and clicks go through the window, and the test
checks the queue the box keeps, so every client would show the same.

## 2. Prove it bites

A new test must fail on the old code, for the right reason. Check it cheaply:

- Revert the one line of the fix (or comment out the feature's call), run the test, see it fail
  with the message you expect, put the line back.
- Or run the new test file in a checkout of main (`git worktree add`, copy the test file in).

A test that passes with the feature deleted protects nothing.

A user's bug: reproduce it the way they saw it where you can (a demo copy, their log or steps). A
fix to a nearby test isn't a fix to their bug. Until the user confirms it in their own copy, report
it as merged and not yet seen, not as fixed.

## 3. Use the real components

Stub only what's outside or unpredictable: the network, the engine's HTTP API, SSH, a coding
agent's CLI, the clock. Everything of ours runs for real. The repo already has stand-ins for those:
`codex-stand-in.js` and the stand-in engine in `box/bridge-tests`, `kuvo::stand_in::StandIn` in Rust.

- No mocks of our own modules.
- Never assert that a stub returned what it was given. Assert what our code did with it.

## 4. Go through the real boundary

Drive the feature the way a client or user does: a route (`PATCH kuvo/bots/…`), a protocol message,
the box's public functions, the app's view state and window events. Not private helpers. A test
through the boundary survives a refactor; one through the insides breaks on every refactor and
still misses the real bug.

## 5. One behaviour per test, named as the scenario

Name it in plain words, as the user would describe it:

- "a queued message survives a bridge restart and is delivered once"
- not "test_queue_2" or "handles restart correctly"

A file opens with a short comment saying what it covers and what's real and what's a stand-in
(see the top of `box/bridge-tests/agents-renewal.test.js`).

## 6. Exact, meaningful assertions

- Check the whole relevant result: the status and the answer's fields, the exact text shown, the
  count ("delivered once" means `toHaveLength(1)`, not "at least one").
- "No error" or "is truthy" is not an assertion about a user outcome.
- When a test fails, first decide whether the code or the test is wrong. Never loosen an assertion
  about what the user gets to make it pass.

## 7. The failures users actually hit

Cover the paths people meet: a restart mid-action, a computer offline, a slow box, an expired
sign-in, a retry, a duplicate delivery, a full disk (`box/test/disk-fault.ts`). Skip exotic edge
cases that need several coincidences.

## 8. Deterministic

- No sleeps, retries or wall-clock waits. Inject time (`workspace.set_now(NOW, cx)` in Rust; pass
  `now` into the box function) and wait on events or a condition (`cx.run_until_parked()`; a bounded
  `until(condition, "what")` helper that fails with a message).
- Each test runs alone, in its own temp home, with every `KUVO_*` it doesn't set unset. Never the
  real `~/.kuvo-code`: `box/test/safety.ts` and `bridge-tests/test-safety.js` refuse it and refuse your
  real coding agents.
- Stop every process you start, by its PID; remove your folders (`removeAtEnd` in the bridge tests).
  Test boxes die with their run (`box/test/preload.ts`).
- A flaky test is a bug: fix the cause, never retry it.

## 9. Every agent

When behaviour differs per agent (Claude Code, Codex, Antigravity, Grok, Cursor, the engine), cover
each kind, or say in the test file which can't apply and why. The same for Mac box versus Linux
machine (route scenarios: `"only": "mac"` with a `"why"`).

## 10. Few and strong

A few tests that catch real breakage beat many that don't. Skip trivial getters, wiring, and pixel
nitpicks: shots cover looks, and a visual fix gets a look before and after.

## 11. Running

Only what you changed and what uses it, never the full suite unless asked. Run them locally
through `scripts/heavy.sh` (the scripts below take its slots). Linux checks run on lokum (`scripts/lokum-box-check.sh`,
`scripts/lokum-bridge-check.sh`), never hetzner; Windows checks with `scripts/windows-box-check.sh`
and `scripts/windows-bridge-check.sh`.

```sh
scripts/cargo-box.sh test -p kuvo-box --test info      # Rust box
scripts/test-box.sh box test/conversation.test.ts      # box
scripts/test-box.sh bridge agents-renewal.test.js      # bridge
cd gpui && ../scripts/cargo-locked.sh test -p kuvo --test send_while_working   # Rust
```

The box and remote test scripts build their inputs with the Bun version in `box/.bun-version`.
For the Rust vectors test, run `scripts/route-shapes.sh` first. For a test that needs an
older bundle, set `BUILT_AT_CACHE` to the test's scratch folder and run
`scripts/built-at.sh <rev>`. It prints the bundle directory.

Run compile checks for what you touched (`cargo-locked.sh check` or `bun run check` in `box/`).
Never drive the Mac app's window from a test.

### Box conformance

Build the TypeScript inputs with `scripts/build-built.sh` and the Rust binary with
`scripts/cargo-box.sh build -p kuvo-box`. Use Node 24. For a TypeScript bundle the adapter runs
Bun. Set `KUVO_CONFORMANCE_BUN` to the engine or Bun executable if needed.

```sh
KUVO_BOX_BIN=box/dist/kuvo-link.js node --test protocol/conformance
for bin in box/dist/kuvo-link.js box/target/debug/kuvo-box; do KUVO_BOX_BIN="$bin" KUVO_CONFORMANCE_AREA=bots node --test protocol/conformance; done
KUVO_BOX_BIN_A=box/dist/kuvo-link.js KUVO_BOX_BIN_B=box/target/debug/kuvo-box node protocol/conformance/differential seed-home requests.json
```

Run the tagged-home and holder cases with `KUVO_CONFORMANCE_AREA=compat` and `KUVO_CONFORMANCE_AREA=holder`; see [fixture generation and reproduction](../protocol/conformance/compat/README.md).

For the differential runner, use an offline scratch home with synthetic data and no process
records. A sequence is a JSON array such as `[{"method":"GET","path":"kuvo/info"}]`. Use
`{{home}}` in a body or query to refer to each run's copy. The runner returns 0 for a match,
1 for a difference and 2 for an input or launch error. For a Claude Code conversation, see
`protocol/conformance/differential/claude-conversation.mjs`. See the
[conversion list](architecture/rust-box.md#3-the-contract-the-rust-box-meets).

## Anti-patterns

- **Snapshot everything**: a giant snapshot gets re-recorded on every change and asserts nothing in
  particular. Name the fields that matter.
- **Asserting calls**: `expect(sendSpy).toHaveBeenCalledWith(…)` on our own module. Assert the
  message that arrived instead.
- **Passes with the feature deleted**: e.g. only checking that a route returns 200, or that a list
  "contains" an item the fixture put there.
- **Sleeping for "eventually"**: `await sleep(500); expect(…)`. Wait on the event or condition.
- **Catch-all**: `try { … } catch {}` around the action, or a test that accepts either outcome.
- **Order and the machine**: a test that needs another to run first, or reads the real
  `~/.kuvo-code`, `HOME`, the network or the time of day.
