# Changelog

## Unreleased

- Quotes work as in 0.7: each part you add to the chat has its own answer under it (click a quote to answer it), so you can comment on each one, and your own words go after them.

- While the agent works, Return adds your message to its task (it reads it at its next step) and ⌘Return queues it for when it's done, as in 0.7; the menu beside Send has both, and Send Now, which stops its current step and gives it your message. Each waiting message has Steer, Send or Send Now, and a right-click menu; a stopped queue says so, with Resume. Every device shows the same queue.

- A message with a picture sent to another computer over a slow connection no longer says "Not sent" when it arrived: the connection waits for it to finish going out, and a message whose answer was lost to a dropped connection is sent again under its own id, so it never goes twice. A connection that drops for a moment comes back quietly, without "Couldn't connect" lines in the conversation.

- A message to another computer over a slow or patchy connection keeps going until it gets there. The connection no longer drops while screenshots are still going out, and a message whose connection dropped (Wi-Fi coming and going) is sent again for up to two minutes, under its own id so it arrives once, showing "Sending…" meanwhile. "Not sent" now means it really couldn't go. The app notes each dropped connection and why in `link.log` in its folder, to send with a report.

- Big pictures (a Retina screenshot, say) go to your other computers the way the iPhone app sends them: a picture over 4 MB goes as a JPEG at most 2048 px wide, so a message with screenshots goes in seconds. A smaller picture goes as it is.

- A message on its way keeps its shape: "Sending…" sits on its own line under it, not over its last line.

- Loading now shows one quiet message in the middle of what's loading, only if it takes a moment; Work Queue headers no longer change height; double-clicking a title-bar button no longer zooms the window.

- Desktop controls now use consistent Lucide icons throughout the app.
