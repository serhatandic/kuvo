# Settings flows: the pattern (#727, MCP Servers first)

Simple settings are rows (`kuvo/settings`, PROTOCOL.md "A box's settings"). Flow sections are a
list of things you add, edit, remove, turn on and off, and sometimes sign in to. This adds the few
kinds that need, in the same vocabulary, so a client draws every flow section without knowing
what any of them is. MCP Servers is the first section built on it.

## The kinds (box/src/shared/routes/settings-pages.ts)

**A list item as a thing** (any row, mostly a `list`'s items):
- `badge: {words, tone}`: its state, beside its title ("Connected", "Needs sign-in"). Tones:
  `quiet`, `good`, `busy`, `warning`, `failed`. A client draws a dot of that tone and the words.
- `notice: {words, tone}`: one more line under it, set apart (the error a server failed with).
  `quiet` or `warning`; selectable, up to a few lines.
- `actions` on a `switch` row: drawn before the switch (Edit…, Remove…, Sign In…). So an item is
  "on/off + what it offers", as Zed's and the Mac's server rows are.

**A form** (`action.form`): an action that asks for values first (Add Server…, Edit…).
- `form: {title, submit, fields, note?}`; each field `{id, title, kind, line?, value?,
  placeholder?, choices?, mono?, folder?, pairs?, shown?, can?}`.
- Field kinds: `text` (one line), `lines` (several, `mono` for a command), `choice` (a segmented
  pick of `choices`), `switch`, `folder` (the box's picker, `folder` words), `pairs` (names and
  values, `pairs: {key, value}` placeholders; value `[{key, value}]`).
- `shown: {field, is: [ids]}`: drawn only while that `choice` field is one of these (On the web
  shows URL and Headers; On this Mac shows Command, Working folder, Environment).
- Checking as you type: `POST kuvo/settings/<section>/<row>/<action> {was, values, check: true}` →
  `200 {fields: [{id, words, tone}], ready}`: the box's words under each field ("It'll be called
  files.", "npx isn't on lokum. It comes with Node.js: install that there first.", "That isn't a
  web address."), `tone` `quiet`, `good` or `warning`, and whether Submit can be pressed (`ready`).
  Nothing is changed. A client asks again a moment after typing stops, and on opening the form.
- Submitting: the same route `{was, values}` → the page (`200`, the form closes), or `400
  {message, fields}`: the form stays, each field's words under it. `409 {page}` stale as ever.

**A flow the box leads** (`row.flow`, on the row or item it's for): a sign-in in the browser.
- `flow: {id, state, title, line, opens?, code?, actions}`. States: `waiting` (open `opens`,
  finish there), `done`, `failed` (`line` says why, in the box's words).
- Started by an action (Sign In…). The box starts it, keeps it (`run/settings-flows.json`, so any
  of its processes states it), and asks the engine how it's going each time the page is read.
- The client that pressed the action opens `opens` itself, once; another window or desktop draws Open Sign-In Page, an action with `opens`.
- `code: {title, placeholder}`: where the page you finish on can't reach the box's computer (another desktop on a different computer), you paste the address it ended on: `POST …/<row>/<action> {was, values:
  {code}}` with the flow's action that takes it (`finish`).
- Its `actions`: `cancel` while waiting; `dismiss` once done or failed; `again` once failed.
- Done: the box does what follows (connects the server again everywhere its engine runs one).

**Live**: nothing new. `kuvo/settings/<section>/events` sends the page each time it changes: the
section's `look` covers the settings file, the flows file, and a second's tick for what's only in
the engine (a server's status, a sign-in's progress).

## How a client draws them (gpui/kuvo/src/settings/)
- An item: dot + badge words beside the title; line under; notice under that (warning colour when
  it says so); its actions as small buttons, then the switch, as now.
- A form: a sheet over the page (the sign-in sheet's place), the box's title, its fields in order
  with `shown` applied, the box's checked words under each field, Cancel and the box's `submit`
  word; Submit off until `ready`. Values are kept in the sheet while it's open (a kind switched
  away from and back keeps what you typed).
- A flow: under its row, a spinner while `waiting` with the box's line and its actions, the code
  field where there's one; the box's words when `done` or `failed`.
- Nothing in the client knows MCP: ids, words, checks and what a submit does are the box's.

## MCP Servers (`mcp`, box/src/link/areas/settings-mcp.ts)
- Servers (list, `add` = Add Server… form): each yours in `mcp.servers` (not Kuvo's own
  `kuvo-browser`, not an integration's, which Integrations keeps), a `switch` (on/off = its
  `disabled`), line its command or URL, badge from the engine's `api/mcp` status, notice its error;
  actions Sign In… (needs a sign-in, with its integration), Try Again (failed), Edit… (form),
  Remove… (confirm). Engine down: the servers still listed from the settings, a warning note.
- Popular servers (`kuvo/mcp/catalog`), grouped by how they're reached, each Add or Added.
- Timeouts: three `number` rows, `mcp.timeout.*`.
- Writes go through `kuvo/config` with `was` (the server's entry as read); 409 → stale page.
- The model is reusable for the Marketplace's Apps (#750): a catalog entry's `add` is an action on
  its item, the same as an integration's would be.

## What each later flow section needs
- Skills, Commands, Custom Auto: a list + Add/Edit forms (`text`, `lines`, `choice`), a check for
  the name; Remove with confirm. No flow.
- Engine: rows + forms for its key-value pieces (`pairs`).
- Accounts/Models: list items with badge + actions; Sign In… as a flow (the same `flow`, with
  `code` for device codes: `line` carries the code); a key as a form with a `secret` field.
- Vault: list + forms; reveal/copy stay in the client's own sheet (an approval flow if it's moved).
- Historical phone pairing used a pairing code and QR address. Its wire support is retained for installed clients.
- Usage: rows (`meter`, switches): no forms.

## How maintainers build a section
1. A module `box/src/link/areas/settings-<id>.ts` exporting a `Section & { listed }` (copy
   settings-mcp.ts's shape), listed in settings-pages.ts (`SECTIONS` and `PAGES`). Helpers are in
   settings-kit.ts: `row`, `item`, `asked`, `refusal`, `STALE`, `staleOrRefusal`, `unreadPage`.
2. Read through the box's existing routes (`asked`), write through them with `was`; map their 409
   to `STALE`. Never a second copy of their rules.
3. Forms: build each with its current values (the box computes `form.revision`). One function reads
   the values for both `check` and the submit (`readForm` in settings-mcp.ts), so the words under
   a field and the refusal are the same. Field words: `warning` holds Submit only through `ready`.
4. What the engine says from moment to moment (a status, the actions that follow from it: Try
   Again, Sign In) goes on the row after `item()` builds it, so it's outside the revision
   (settings-mcp.ts, `now`). Anything that differs by who asks (`fromApp`) must be outside the
   revision too.
5. A flow: `keepFlow` on start, follow it in `read` unless `quiet` (a check reads quietly), `endFlow`
   once (its `true` says you're the one to do what follows), `dropFlow` on cancel/dismiss. Only keep
   an `opens` that's http(s).
6. Tests (box): the section rules note's list, plus: a check's words, a submit refused with the same
   words, a form not stale from another item's change, the flow's states. Desktop: the stand-in's page
   for the section (gpui/kuvo/src/stand_in/settings/<id>.rs) and scenario tests through the window;
   shots light and dark.
7. One line in FEATURES only if the section needs a new kind; else none.

## Status

The box owns MCP Servers and its form checks. GPUI draws its described pages
for reachable boxes, including sign-in, field errors and stale form handling.
The retired clients remain compatibility readers. A box's feature and
compatibility facts decide which controls can be drawn. See
[Settings](../../gpui/notes/screens-settings.md) and the
[protocol](../../protocol/PROTOCOL.md).

## Repeated sections (2026-11-16)
Skills, Commands and Custom Auto repeat the list/forms pattern, using existing kinds only.
Custom Auto is the ordered model fallback lists (`api/kuvo/auto`), not Permissions' allow/deny.
Its effort choices use `shown` for the selected model; no new kind was needed.
Skills installs through the reusable `kuvo/skill-library/install` route, accepting a repository
name or its main GitHub URL plus the skill's folder. Branch/file-qualified links are refused
explicitly rather than silently installing from another ref. Repo browsing and file previews
belong to Marketplace Skills (#750), by Foreman's decision; do not extend Settings for them.
The retired Swift application stays read-only. The desktop draws each page generically; fixed
stand-in descriptions exercise the existing list/form renderer without duplicating section logic.
