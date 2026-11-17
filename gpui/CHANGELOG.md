# Changelog

## Unreleased

- In the desktop app's title bar, the sidebar button, the computer picker and back and forward now sit together over the sidebar, with a thin divider where the sidebar ends, and the page title starts after it. The extra + beside them is gone: New Session is still ⌘N and File › New Session, and each area keeps its own +.

- In the desktop app, hover over any message, yours or the agent's, to see when it was written (the day too, in a tooltip, when it isn't today) and buttons to copy it as Markdown or add it to the chat.

- In the desktop app, **Add to Chat** now shows over the end of what you select as soon as you let go, not only in the right-click menu. Esc, a click elsewhere, or using it puts it away.

- In the desktop app, the bots in the Bots sidebar are bigger (40 points, with taller rows), so their faces and moods read at a glance. Bots elsewhere (the header, mentions, pickers) keep their sizes.

- In the desktop app, menus and cards open from what you clicked, as on macOS: a button's menu right under it, lined up with its edge; the composer's menus just above their control; a usage ring's card to its right, level with its top; a right-click menu at the pointer. Near the window's edge, a menu opens up or to the left instead of covering what opened it, and always stays fully in the window.

- First run is remembered on this desktop device. Switching computers opens the app directly, including computers without projects, and Get Started now visits each setup step in order.

- In the desktop app, the sidebars and title bar read more calmly. Every icon in the rail, title bar, toolbars and rows has the same line weight, and the open area's icon no longer looks bolder than the rest. Bots', computers' and waiting sessions' names are set a step stronger than the line under them, which is smaller. Only "waiting on you" is coloured, in a softer blue; "1 at work" stays grey. A selected row's background is lighter. In the title bar, the window's title is the strongest text, the computer's name is quieter, and the sidebar button no longer shows a filled tile.

- In the desktop app, the bar over a bot's composer no longer shows how many tokens the day has used ("4 sessions at work · answered 1 ask"); that will live in Insights.
- Quotes work as in 0.7: each part you add to the chat has its own answer under it (click a quote to answer it), so you can comment on each one, and your own words go after them.

- While the agent works, Return adds your message to its task (it reads it at its next step) and ⌘Return queues it for when it's done, as in 0.7; the menu beside Send has both, and Send Now, which stops its current step and gives it your message. Each waiting message has Steer, Send or Send Now, and a right-click menu; a stopped queue says so, with Resume. Every device shows the same queue.

- A message to another computer over a slow or patchy connection keeps going until it gets there. The connection no longer drops while screenshots are still going out, and a message whose connection dropped (Wi-Fi coming and going, or its answer lost on the way) is sent again for up to two minutes, under its own id so it arrives once, showing "Sending…" meanwhile; Stop takes back one that hasn't gone yet. "Not sent" now means it really couldn't go. A connection that drops for a moment comes back quietly, without "Couldn't connect" lines in the conversation. The app notes each dropped connection and why in `link.log` in its folder, to send with a report.

- Big pictures (a Retina screenshot, say) go to your other computers the way the iPhone app sends them: a picture over 4 MB goes as a JPEG at most 2048 px wide, so a message with screenshots goes in seconds. A smaller picture goes as it is.

- A message on its way keeps its shape: "Sending…" sits on its own line under it, not over its last line.

- Loading now shows one quiet message in the middle of what's loading, only if it takes a moment; Work Queue headers no longer change height; double-clicking a title-bar button no longer zooms the window.

- Desktop controls now use consistent Lucide icons throughout the app.
