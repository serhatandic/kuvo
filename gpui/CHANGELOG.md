# Changelog

## Unreleased

- Computers shows the main computer and **Change…**, with rename and removal actions from the box. Unlink retains local work. Unlink + wipe verifies any required backups and transfers. The last computer offers **Wipe This Computer** and can offer reinstall afterward.

- The title-bar update notice shows preparation or a failure you can act on. Computers shows each box’s status and **Try Again** where available. Your computers keep working while a release stages and follow the main once it switches.

- Accounts uses the sign-in route the computer supports and shows whether it is sending a pasted code, checking the sign-in, or still waiting for confirmation.

- In the desktop app, when a computer's coding agents aren't running, Settings › Accounts, Settings › Usage, the usage rings in the rail and the line over a conversation say why once, in one plain sentence (no more "aren't running on hetzner: The coding agents aren't running…"), with **Try Again** where it can help. While they restart, nothing turns into an error: the rings keep their last readings, shown fainter, and their card says they're restarting.
- In the desktop app (and the menu bar panel's rings), every provider's logo now draws at one size: Claude, OpenAI, OpenCode and the rest no longer look tiny or huge by how their artwork happened to be padded. A logo takes about 63% of the circle or tile it sits in, a round one a touch larger than a square one so they look alike.

- In the desktop app's title bar, the sidebar button, the computer picker and back and forward now sit together over the sidebar, with a thin divider where the sidebar ends, and the page title starts after it. The extra + beside them is gone: New Session is still ⌘N and File › New Session, and each area keeps its own +.

- In the desktop app, hover over any message, yours or the agent's, to see when it was written (the day too, in a tooltip, when it isn't today) and buttons to copy it as Markdown or add it to the chat.

- In the desktop app, **Add to Chat** now shows over the end of what you select as soon as you let go, not only in the right-click menu. Esc, a click elsewhere, or using it puts it away.

- In the desktop app, the bots in the Bots sidebar are bigger (40 points, with taller rows), so their faces and moods read at a glance. Bots elsewhere (the header, mentions, pickers) keep their sizes.

- In the desktop app, menus and cards open from what you clicked, as on macOS: a button's menu right under it, lined up with its edge; the composer's menus just above their control; a usage ring's card to its right, level with its top; a right-click menu at the pointer. Near the window's edge, a menu opens up or to the left instead of covering what opened it, and always stays fully in the window.

- First run is remembered on this desktop device. Switching computers opens the app directly, including computers without projects, and Get Started now visits each setup step in order.

- In the desktop app, the sidebars and title bar read more calmly. Every icon in the rail, title bar, toolbars and rows has the same line weight, and the open area's icon no longer looks bolder than the rest. Bots', computers' and waiting sessions' names are set a step stronger than the line under them, which is smaller. Only "waiting on you" is coloured, in a softer blue; "1 at work" stays grey. A selected row's background is lighter. In the title bar, the window's title is the strongest text, the computer's name is quieter, and the sidebar button no longer shows a filled tile.

- In the desktop app, the bar over a bot's composer no longer shows how many tokens the day has used ("4 sessions at work · answered 1 ask"); Insights shows usage by bot.
- Quotes work as in 0.7: each part you add to the chat has its own answer under it (click a quote to answer it), so you can comment on each one, and your own words go after them.

- While the agent works, Return adds your message to its task (it reads it at its next step) and ⌘Return queues it for when it's done, as in 0.7; the menu beside Send has both, and Send Now, which stops its current step and gives it your message. Each waiting message has Steer, Send or Send Now, and a right-click menu; a stopped queue says so, with Resume. Every device shows the same queue.

- Messages appear immediately in the conversation or queue. A dropped connection or box restart can retry the same send without creating a second message. Connection diagnostics are recorded in `link.log`.

- Big pictures (a Retina screenshot, say) go to your other computers: a picture over 4 MB goes as a JPEG at most 2048 px wide, so a message with screenshots goes in seconds. A smaller picture goes as it is.

- Loading now shows one quiet message in the middle of what's loading, only if it takes a moment; Work Queue headers no longer change height; double-clicking a title-bar button no longer zooms the window.

- Desktop controls now use consistent Lucide icons throughout the app.
