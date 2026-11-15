# Group chats: handover

## Done

Scenarios: `kuvo/tests/groups.rs` (8, real clicks and keys against the stand-in), unit tests in
`kuvo_box/src/groups.rs` and `kuvo/src/groups/chat.rs`. Shots: `shots/screens-group-*-{light,dark}.png`
(10 screens), each looked at; `screens-area-bots` rendered again.

1. **A chat's conversation** (`kuvo/src/groups/chat.rs`), in Bots, picked from the sidebar's Group
   Chats: your messages in your bubble, each bot's under its still face, name and when (a wrap-up
   or a report said so), its steps and words drawn as a session's (`entries::message_body`, shared,
   not forked); a bot that left the chat is still named ("Left the chat"), one no list knows is "A
   bot that left"; the chat's notes and meetings as quiet lines; who's talking now and its last
   step; a wrap-up's parts with their state ("Waiting: X's conversation is stopped" from the box's
   `stopped`) and Hand Out the Work for proposed ones; Keep Going once it paused. One selection,
   ⌘C, ⌘A and the right-click menu, as a conversation's. Read means `…/viewed` (the dot goes).
2. **The composer** is the app's (`Target::Group`): only the words go (`POST kuvo/groups/<id>`),
   the box names who they name. "&" (or "@") offers the chat's bots; a face under the field asks
   that bot ("&Name "). Stop (and Escape in the empty field) stops the discussion. No files, no
   model or effort chips, no "/" list: a chat's bots each have their own. What the box turns down
   stays in the field with its words.
3. **The strip** over a chat: its bots' faces and names, and Edit.
4. **Edit Group** and **New Group** (the + beside Group Chats): Name, What it's for, and the bots
   the box offers (`kuvo/groups/joinable`, another computer's said as "On <computer>"), as
   checkboxes; Save sends only what changed (`PATCH`), Create sends `POST kuvo/groups`; the box's
   answer is drawn after (the chat opens), or its refusal in its words. **Delete** is asked first.
5. `kuvo_box::groups`: typed reads and sends for all of it; `areas::Group` is that one now. Fixed:
   the sidebar read a chat's last time as milliseconds; the box says seconds.
6. The stand-in box answers all of it in the box's shapes (`kuvo/src/stand_in/groups.rs`):
   "Launch crew" (three bots mid-discussion, a message from a bot since deleted), "Design review"
   (paused, a wrap-up, a bot that left it but is still a bot), "Weekly planning" (empty), followed
   live (`…/events`), and with `--stand-in` the bot talking answers a moment after you write.

No box changes.

## Review

One reviewer pass; every MUST and SHOULD fixed or listed, with a scenario where one can be written:
- **MUST:** a bot's reply could go missing: the follow hands on only the latest line, and the
  next one (who talks next) leaves `messages` out. Lines are filled with the last messages on the
  follow's own thread now (`Chat::filled`, `follow_lines_filled`; unit test
  `a_line_without_messages_carries_the_last_ones_said`).
- **MUST:** a chat was marked read while you were in another area. The window tells Bots when
  its main view is on screen (`groups_on_screen`); off screen a chat isn't followed or marked
  (`a_chat_off_screen_isnt_marked_read`, seen failing without the fix).
- Read was tracked by the number of messages (the box sends 300 at most): by the last one's id now.
- A failed read of the chats emptied the list and called every bot "Left the chat": what was last
  read stays, and the sidebar says why only with nothing to show
  (`chats_that_cant_be_read_just_now_stay_as_they_were`).
- The chats' list is followed now (`kuvo/groups/events`, `follow_groups`), and a write in the app
  reads it again.
- Re-reading the list snapped an open chat to its end: rows are redrawn in place, and only when
  who names a message changed.
- Files dropped on a group chat's composer were kept and never sent: it takes no drops, and an
  empty field doesn't send.
- Resume for a part waiting on a stopped conversation
  (`a_part_waiting_on_a_stopped_conversation_resumes_it`); CHANGELOG line, naming what isn't in
  the desktop app yet.
- A chat deleted elsewhere (or Edit of one) says it isn't here anymore, and its follow ends
  (`a_chat_deleted_elsewhere_says_its_gone`).
- An opened chat followed forever: it's followed only while on screen.
- NITs fixed: a bot taken out and put back is no change (`a_bot_taken_out_and_put_back_is_no_change`);
  one `is_group_key`; "is working…" / "is writing…" for who's talking; the blank face for an
  unknown look under the field; `sent_at`'s comment.
- NITs left: the bot picker's rows and the faces under the field aren't focusable (no keyboard
  path), as other clickable rows in the app; "You'll be asked…" kept, as Bots' page says it.

## BOX-NEEDED

- **What "&" offers in a group chat**, ranked by the box: its bots, then the guests it would name
  there (`kuvo/named`, others' bots): `GET kuvo/groups/<id>/mentions?q=` (as
  `kuvo/sessions/<id>/mentions`). Until then the composer offers the chat's own bots, filtered by
  what's typed (`composer.rs`, `member_offers`, marked). No guests are offered.
- Nothing else was stood in for: every other fact drawn is the box's today.

## Left (not built)

- Undo of your message (`…/undo`), Reply (quote) to a bot's message, Copy under the pointer.
- Resume of a stopped part whose bot lives on another computer (asked of this box only).
- A chat isn't marked read while the window isn't the frontmost app (it is while it's on screen).
- Meetings: shown in the chat, but not set or changed (`meeting` in `POST`/`PATCH`).
- `&Name` in bold in the bot's colour in messages (Markdown has no styled runs yet); bots' faces
  in place of "@".
- The "&" list shows a bot icon, not each bot's face (the shared list draws icons).
- A bot's conversation opened from a chat (its name) goes to the bot's page on Back, not the chat.
- Move to Computer; a chat on another computer (the app reaches this Mac's box only); vault asks
  from seats; widgets and pictures a bot made (shown as its steps).
- Find (⌘F) in a chat. VoiceOver labels; larger text not tried.
- The phone: no change (the desktop app isn't shipped). Docs and CHANGELOG: none needed.
- **Left in the client, view words:** "A bot that left", "Left the chat", a part's state words,
  "<name> is thinking…", the intro's sentence, the paused bar's words.

## Decisions for the user

Built without drafts (the lead's brief: build, with shots); each is cheap to change.

- **A chat shows in Bots' main view** with a strip (faces, names, Edit), as a bot's conversation
  does. Recommended: keep.
- **"&" names a bot; "@" opens the same list** but puts in "&Name", since the box reads only "&"
  (`rules.ts`, `mentions`). The Mac's intro says "Write @"; ours says "Write &". Recommended: keep.
- **Edit is a page, not a sheet**, the same form as New Group, with Delete at its foot asked inline.
  Recommended: keep.
- **No "pick at least two" hint**: the box says it when you Create (`screens-group-new-refused`),
  so no rule of the box's is copied into the form. Recommended: keep.
- **The intro** doesn't say who goes first or sums up (the Mac's does): that's the box's rule.
- **New Group's + shows under the pointer**, as Labels' does in the Work Queue; the empty list
  says where it is. Consider always showing it while there are no chats.
- **Wording** throughout ("What it's for: The bots read it as they talk.", "Hand Out the Work",
  "Paused after a good few turns…"): the user's to settle.
