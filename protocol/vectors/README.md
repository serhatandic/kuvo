# Shared vectors

What both the box (`box/`) and the desktop app (`gpui/`) test against, so the two sides can't drift:
the link's keys, frames and sessions (`noise.json`, `link.json`, written by `make-link-vectors.py`),
the browser's live view, message parts, session lists, model readiness and sign-in states, and the
route shapes and features that `scripts/route-shapes.sh` writes (`routes/`). A case only one side
reads belongs with that side's tests instead: the box's in `box/test/fixtures` (its route scenarios
too, in `box/test/fixtures/routes`). `tool-steps.json` is read only by the desktop app for now.

`file-addresses/windows.json` is hand-written. Its wire paths retain the original case, and its
extended Win32 spellings are for local file calls. They are rejected if sent as wire paths.
