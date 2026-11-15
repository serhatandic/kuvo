# Session menu (Draft B): handover

## Done

Built Draft B of the desktop app's session menu against `kuvo_box::session_actions` and the
stand-in box, fitting into `kuvo_ui` and the desktop app's existing design language:

1. **Row hover actions (`gpui/kuvo/src/sidebar.rs`):** Pin/Unpin (`IconName::Pin`) and Archive
   (`IconName::Archive` from `gpui/assets/icons/archive.svg`, registered in `kuvo_ui/src/icon.rs`)
   appear on session rows (`session_row`, `needs_row`) only while the pointer is over the row or
   that row's menu/popover is open — never as invisible hitboxes when unhovered. When the Archived
   filter is active, hover actions switch to Unarchive (`IconName::Undo`) and Delete…
   (`IconName::Trash`). Buttons are separated (`gap(space::S2)`, `pl(space::S1)`) and swallow left
   mouse-down/click events so a narrow-row slip never opens the row.
2. **Box-driven full menu (`gpui/kuvo/src/session_menu.rs`, `sidebar.rs`, `title_bar.rs`):** Right-click
   on any session row (`session_row`, `found_row`, `needs_row`) via `ContextMenuArea::with_handle`
   and the `"•••"` (`session-more`, `IconName::More`) `PopoverMenu` at the right of the title bar
   open the same `Menu` populated from `GET kuvo/sessions/<id>/actions`: box order, box labels,
   `"…"` added for actions that prompt (`rename` or `confirm.is_some()`), a separator before the
   first `destructive` action, danger styling, shortcut hints (`⌘R`, `⌘⌫`, `⇧⌘⌫`), unknown IDs
   skipped, and `held` words heading the menu with all entries disabled.
3. **Inline popovers (`gpui/kuvo/src/session_menu.rs`):** `300 px` elevated popovers anchored under
   the row (`Anchor::TopLeft`) or the title bar's `"•••"` button (`Anchor::TopRight`), snapping to
   window bounds:
   - **Rename (`RenamePopover`):** Pre-selected bare `TextField` in an accented box; `Return`, the
     primary Rename button, and clicking outside save; `Escape` and Cancel discard; empty or
     unchanged names close without calling the box; box refusals (`409` etc.) stay open inline.
   - **Delete confirm (`ConfirmDeletePopover`):** Exact box `confirm { title, message, label }`
     (including worktree / uncommitted changes warnings such as `ses_dark`'s); refusals stay open
     inline; confirmed deletes clean up open workspace state and return to Landing if the deleted
     session was open.
4. **Pinned ordering & Archived filter (`gpui/kuvo/src/sidebar.rs`, `workspace.rs`, `title_bar.rs`):**
   - Removed the separate `Pinned` section; pinned sessions appear once, raised to the top within
     their own project (before `SHOWN = 6`) and within `Chats`, marked with a leading `IconName::Pin`
     when not unsent, running, or asking.
   - Added an Archived filter toggle (`IconName::Sliders`) inside `sidebar-search` (placeholder
     switches to `"Search archived"`), a removable `"Archived"` chip (`showing-archived`) below
     search, grouped archived rows by project + `Chats` (newest first), archived-only search
     filtering, an `"Archived"` chip (`is-archived`) in the title bar when an archived session is
     open, and a dismissible `session-action-error` banner for refusals outside popovers.
5. **Shortcuts & selection look (`gpui/kuvo/src/session_menu.rs`, `row.rs`, `rail.rs`, `areas.rs`):**
   Bound `⌘R` (Rename), `⌘⌫` (Archive), and `⇧⌘⌫` (Delete…) in `Workspace` (while focused
   `TextField`s keep `⌘⌫` for deleting to line start), targeting the walked sidebar row when walking
   and otherwise the open session. Reverted `selected_card` to `c.selected` wash across sidebar rows,
   the rail, and area sidebars.
6. **Stand-in box (`gpui/kuvo/src/stand_in.rs`), tests (`gpui/kuvo/tests/session_menu.rs`), and shots (`gpui/shots/screens-session-menu-*.png`):**
   Added `GET kuvo/sessions/<id>/actions`, `POST …/pin`, `…/archive`, `…/rename`,
   `DELETE kuvo/sessions/<id>`, and `POST kuvo/sessions/deletes` plus test state helpers
   (`hold_session`, `set_engine_archived`, `set_kept_worktree_changes`). Added 6 integration tests
   and 8 light/dark screenshots (`row-hover`, `row-menu`, `title-menu`, `rename`,
   `delete-worktree`, `pinned`, `archived`, `held`).

## Zed reference files used (ideas only, never copied)

Read from `.zed-ref/crates` for interaction patterns and GPUI architecture only:

- `.zed-ref/crates/sidebar/src/sidebar.rs`: Conditional rendering of hover action slots only when
  hovered or when that row's menu/popover is open; tracking open `PopoverMenuHandle` state to keep
  row highlight and controls visible while the menu is open; inline rename focus/blur commit and
  Escape cancel lifecycle.
- `.zed-ref/crates/ui/src/components/ai/thread_item.rs`: Keeping action buttons out of the DOM when
  unhovered and stopping left `mouse_down` propagation on trailing action buttons so clicks never
  bleed through to the underlying session row.
- `.zed-ref/crates/ui/src/components/right_click_menu.rs` and
  `.zed-ref/crates/ui/src/components/popover_menu.rs`: Deferring focus into elements mounted inside
  `deferred(anchored())` popovers until after layout, snapping overlays within window margins
  (`snap_to_window_with_margin(px(8.))`), and restoring focus to the prior target when closing
  without committing.
- `.zed-ref/crates/agent_ui/src/threads_archive_view.rs`: Archived list behavior with muted row
  presentation and direct Unarchive / Delete icon actions on archived rows.
- `.zed-ref/crates/title_bar/src/title_bar.rs`: Holding a `PopoverMenuHandle` on a small ghost icon
  trigger in the title bar so active/selected button state reflects menu/popover openness.

## BOX-NEEDED

1. **Whether an archived session can still be written to (`gpui/kuvo/src/session_menu.rs`):**
   `GET kuvo/sessions/<id>/actions` / session readiness does not state whether sending a message to
   an archived session is allowed or disabled, so the composer stays open on archived sessions today.
2. **Structured Delete confirm facts (optional, same as noted in drafts):** `confirm.message`
   currently carries a single pre-worded sentence from the box (`"Its worktree and changes that
   weren’t committed are deleted too."`), which Draft B displays verbatim. If a future layout lists
   worktree name and uncommitted file counts separately, the box would need to return those facts.

## What's left

- No bulk "Delete All…" UI is included in Draft B (though `POST kuvo/sessions/deletes` is already
  implemented in `kuvo_box` and `StandIn` should a footer/header control be desired later).
- Live network integration against the real box (verified against `StandIn` in `cargo test` and
  offscreen screenshots).

## Review

Reviewed internally before handover and then in the Foreman review round:

1. **Sequential live action tasks (`gpui/kuvo/src/session_menu.rs`, `workspace.rs`):** Replaced the
   single `SessionMenu::action_task: Option<Task<()>>` slot with `action_tasks: Vec<Task<()>>` so
   triggering two quick actions (e.g. Pin then Archive) no longer drops and cancels the first task.
2. **Archived filter toggle clearing active popovers (`gpui/kuvo/src/workspace.rs`,
   `tests/session_menu.rs`):** `set_archived_filter` now clears `session_menu.active_popover`.
3. **Click-away during live popover submit/delete (`gpui/kuvo/src/session_menu.rs`):** Added a
   shared `Rc<Cell<bool>>` (`restore_focus`) on `RenamePopover` and `ConfirmDeletePopover` so
   clicking outside while a live request is in flight preserves user focus.
4. **Foreman review fixes (3 MUST, 1 SHOULD):**
   - **MUST 1 (Active search excludes archived):** Filtered both `named` and `in_conversations` in
     `sidebar.rs` `found_lines` with `listing.facts(id).is_some_and(|f| f.is_archived() == archived_only)`.
   - **MUST 2 (Box-driven hover buttons):** Cached `GET kuvo/sessions/<id>/actions` per session
     in `SessionMenu::row_actions` (epoch-guarded, refreshed on hover/menu/action/listing change)
     and rendered row hover buttons purely from box actions — hiding on `away`/`held`, omitting
     `Unarchive` on engine-archived sessions, and removing all client-side `if archived` rules.
   - **MUST 3 (Confirm-only deletion):** Removed the direct-delete branch from
     `Workspace::run_delete_action`; deletion now runs solely through
     `ConfirmDeletePopover::confirm_delete`, synthesizing a plain title-only confirm when the box
     gives `confirm: None`.
   - **SHOULD (Shortcuts & popover cleanup):** Bound `⌘R` / `⌘⌫` / `⇧⌘⌫` in
     `Workspace && !KuvoTextField` so text inputs ignore them while walked rows (`Sidebar`) still
     work; cleared `walked` / `hovered_row` in `archive_session` and returned `None` from
     `shortcut_target` when `walked` is missing from `sidebar::lines`; and cleared/pruned
     `active_popover` when its anchor leaves (`new_session`, `open_session` including the already-open
     session, `search_changed`, `clear_search`, `toggle_project`, `toggle_all`, `show_area`,
     `rail_pressed`, and `take_list`).

