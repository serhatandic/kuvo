# Rust builds and tests

From `gpui/`, use `../scripts/cargo-locked.sh <cargo arguments>`. Builds, tests, checks, Clippy,
and shots all take the same single Rust slot for this user, wherever the checkout lives.
Use `scripts/heavy.sh rust <command>` for multiple steps that need the same slot.
`scripts/heavy.sh status` shows the running job and the queue.

Keep `CARGO_TARGET_DIR` per checkout. On the 12-core Mini, Cargo 1.98.1 reused checkout A's
executable when building the same package from checkout B with a shared target. That executable
still reported A's compile-time `CARGO_MANIFEST_DIR`. Our shots binaries use that path for their
output, and some tests use it to find fixtures. Cargo's locking serializes access to the target
but cannot correct that path.
