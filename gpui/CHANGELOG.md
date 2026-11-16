# Changelog

## Unreleased

- A message with a picture sent to another computer over a slow connection no longer says "Not sent" when it arrived: the connection waits for it to finish going out, and a message whose answer was lost to a dropped connection is sent again under its own id, so it never goes twice. A connection that drops for a moment comes back quietly, without "Couldn't connect" lines in the conversation.

- A message on its way keeps its shape: "Sending…" sits on its own line under it, not over its last line.

- Loading now shows one quiet message in the middle of what's loading, only if it takes a moment; Work Queue headers no longer change height; double-clicking a title-bar button no longer zooms the window.

- Desktop controls now use consistent Lucide icons throughout the app.
