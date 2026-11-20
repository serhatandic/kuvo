# Antigravity 1.3.0 model discovery (#740)

Inspected the installed darwin-arm64 `agy_acp_server.par` read-only, without
executing it or reading any profile/sign-in files. SHA-256:
`cb1f0725183ed922079ed6cd1d2422b4d8be44c5fb60be3e9a43d8d04f99a18e`.
The executable contains ZIP local entries with embedded Python source; those
entries were read/decompressed in memory, never imported or executed.

Evidence under `google3/cloud/developer_experience/antigravity_extensions/acp_server/`:

- `model_selection.py`, `supports_third_party_models`: returns
  `client.is_jetbrains or client.is_zed or client.is_xcode`.
  Its comment explicitly says unrecognised clients keep the Gemini-only list.
- `_parse_ccpa_response`: iterates `agentModelSorts` model IDs and discards IDs
  not starting with `gemini` unless `allow_third_party` is true. Names come
  from `models[id].displayName`. Actual model IDs depend on the service reply;
  this inspection does not establish which Claude/GPT versions are entitled.
- `get_available_models` calls `fetch_available_models` on the OAuth path and
  passes `supports_third_party_models(client)` to that parser.

Kuvo sends `clientInfo.name = "kuvo-code"` (both sign-in in
`src/agents/agents/antigravity.ts` and ordinary ACP initialization). Thus this
agent version does not expose Claude/GPT models to Kuvo. No client identity
spoofing or speculative model IDs were added.

`session/new` feeds `saveModels`: the box preserves model config options,
including grouped options, falling back to `models.availableModels`. The saved
list is returned by `models()` without a Gemini-only filter in Kuvo itself.

Quota reading is independent: `retrieveUserQuotaSummary` supplies exact
`gemini-5h`, `gemini-weekly`, `3p-5h`, `3p-weekly` bucket IDs plus the separate
unambiguous weekly bucket in the `Gemini 4 Argon` group. Existing windows remain
independent and now explicitly name Gemini, Claude and GPT, and Gemini 4 Argon.
The legacy fallback supplies only the short pools; missing weekly data is not
invented. `limits-rows.ts` preserves all window titles and pairs overview rings
within the pool closest to a limit. No protocol shape or feature gate changed.
