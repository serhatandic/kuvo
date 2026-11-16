extension BoxGate {
    /// What this client uses with no way around it on a box that hasn't it. A name goes in here
    /// when the client drops its fallback for a box without it, or would say it needs a newer
    /// Kuvo Code; the same names in gpui's `kuvo_box` (src/gate/required.rs) and
    /// protocol/gate-required.txt (PROTOCOL.md, "The update gate"), which gpui's tests hold to this.
    ///
    /// One name a line, in order (`LC_ALL=C sort`), what needs it in a comment on that line, and
    /// nothing else changes in this file: git merges it by union (.gitattributes), so branches that
    /// each add a name merge without a conflict, two added side by side in either order
    /// (CONTRIBUTING.md, "What branches don't edit by hand").
    public static let required: [String] = [
        "account-action-sheet-title", // #734
        "agent-tools.own",
        "auto.lists",
        "bots.deletes", // the desktop client's Bots area draws what the box says of a delete
        "bots.effort", // the Bots area: a model's effort, as the box says it
        "bots.elsewhere", // the Bots area: the bots on your other computers
        "bots.held", // #668 item 8: an edit to a bot whose computer is away is held by the box asked; no client keeps an outbox
        "bots.now", // #711, the desktop client's sidebar and Settings draw what the box says
        "bots.state", // a bot's one state (thinking, blocked, done, asleep…) and Seen are the box's; no client works one out
        "clients.kind", // #742: a client says what it is, so a desktop app isn't taken for a phone
        "computers", // #711
        "computers.add", // #636: a box adds a computer over SSH itself, and says its steps and questions
        "computers.facts", // #735: what each computer answers, which of its sessions are at work
        "computers.manage", // #735: renaming, removing and reaching one again from any box, as its `actions` say
        "computers.reach", // #742: where each of your computers is reached, for a client that connects to each directly
        "computers.reach-words",
        "computers.words",
        "devices.allow", // #742: a computer whose app was removed adds none until you allow it again
        "devices.pair", // #742: the desktop app pairs through the box on its own computer
        "file.text", // #708, the side panel and the sandbox's row work nothing out from what the box lists
        "folders.suggested", // #733, the facts a first run draws, which no client works out
        "folders.tree",
        "git.branches",
        "git.branches.pullRequest", // #708
        "git.branches.worktree", // the desktop client's side panel: Branches
        "git.changes.binary", // #708
        "git.changes.path", // the desktop client's side panel: Changes
        "git.files", // the desktop client's side panel: Changes
        "git.files.does", // #708
        "git.merge",
        "git.pull-request",
        "git.status",
        "groups.mentions", // #736: a group chat's "&" lists what the box offers there (its bots, then guests); no client filters
        "groups.task-title",
        "limits.bar", // #668: the ring one bar shows, and a window that started over, as the box says
        "limits.rings", // #711
        "limits.titles", // #668: every account and window named by the box
        "messages.commands", // #668: a command's message read as typed, by its id
        "messages.compaction-words",
        "messages.send", // #668 item 3, a message sent as fields; a box with it has what came before it too (`sessions.list`, `typed`, plan limits' `rows`, `kuvo/updates`, Cursor's limits, `kuvo/sessions/read`)
        "models.offered",
        "models.offered.folder",
        "models.offered.free", // #733
        "models.offered.sides",
        "models.readiness",
        "names.titles",
        "queue.items", // the desktop client draws the queue as the box says it
        "queue.removed.keyword", // #668: a queued message taken back keeps its keyword
        "queue.stop.arriving", // #668: Stop holds what's still on its way, so no client waits for it
        "sandbox.needsYou", // #739: whether the sandbox's row warns, the box's one fact
        "sandbox.opens", // #708
        "sandbox.report", // #668: a machine's services gap in the box's words, and when to offer Report a Problem
        "sandbox.words", // #711
        "sessions.actions",
        "sessions.asks", // the desktop client draws asks as the box says them
        "sessions.asks.box", // and answers the sandbox's and the vault's asks there
        "sessions.archived-groups",
        "sessions.list",
        "sessions.mentions", // the desktop client draws "@" and "&" as the box says them
        "sessions.models",
        "sessions.needsYou", // #711
        "sessions.start.empty",
        "settings", // #727: a box describes its own settings (`kuvo/settings`)
        "settings.flows", // #727: forms, checks and flows in `kuvo/settings` (MCP Servers)
        "settings.vault", // A computer's Vault in `kuvo/settings`: secret and file fields, Show
        "setup.ready", // the first run's skip, decided by the box
        "setup.welcomed", // #733
        "sign-in.accounts.kinds", // #711
        "sign-in.accounts.plan", // #733 (with a key-only account's Add Key)
        "sign-in.accounts.state", // #668: no client decides when Disconnect, Sign In Again or Install show
        "sign-in.codes", // #711
        "typed",
        "usage.notices",
        "waiting", // #711
        "waiting.cards", // #718: what waits on you counts the cards a client used to add itself
        "work-queue.anywhere", // #668: an item from any box; no client asks each computer for one
        "work-queue.counts", // #711
        "work-queue.edit", // #674: the Work Queue's statuses, priorities and labels in place of categories
        "work.background.items", // #764: the bars over the composer list what runs for a conversation from the box
    ]
}
