//! Tool and integration words used by the existing message protocol.

pub(super) const BUILT_IN: &[&str] = &[
    "bash",
    "shell",
    "read",
    "edit",
    "multiedit",
    "patch",
    "apply_patch",
    "write",
    "grep",
    "glob",
    "list",
    "ls",
    "webfetch",
    "websearch",
    "task",
    "subagent",
    "todowrite",
    "todoread",
    "question",
    "skill",
    "codesearch",
    "lsp",
    "invalid",
    "batch",
    "plan_enter",
    "plan_exit",
    "execute",
    "context",
];

pub(super) fn did_once(name: &str) -> &'static str {
    match name {
        "shell" | "bash" => "Ran 1 command",
        "execute" => "Ran code",
        "read" => "Read 1 file",
        "edit" | "multiedit" | "patch" | "apply_patch" | "write" => "Edited 1 file",
        "grep" | "glob" | "list" | "ls" | "websearch" | "codesearch" => "Searched once",
        "webfetch" => "Read 1 web page",
        "task" | "subagent" => "Asked 1 subagent",
        "todowrite" | "todoread" => "Updated its plan",
        _ => "Used 1 tool",
    }
}

pub(super) const BRIDGE: &[(&str, &str, &str)] = &[
    ("plan", "Proposed a plan", "Proposing a plan"),
    ("memory", "Recalled from memory", "Recalling from memory"),
    ("hook", "Ran a hook", "Running a hook"),
    ("sleep", "Waited", "Waiting"),
    ("think", "Thought it over", "Thinking it over"),
    ("generate_image", "Made an image", "Making an image"),
    (
        "record_screen",
        "Recorded the screen",
        "Recording the screen",
    ),
    ("lints", "Checked for problems", "Checking for problems"),
    ("tool_search", "Looked up tools", "Looking up tools"),
    ("tool", "Used a tool", "Using a tool"),
];

pub(super) const OWN: &[(&str, &str, &str)] = &[
    ("browser_navigate", "Opened a page", "Opening a page"),
    ("browser_snapshot", "Read the page", "Reading the page"),
    (
        "browser_click",
        "Clicked on the page",
        "Clicking on the page",
    ),
    ("browser_type", "Typed on the page", "Typing on the page"),
    ("browser_select", "Chose an option", "Choosing an option"),
    ("browser_scroll", "Scrolled the page", "Scrolling the page"),
    ("browser_press", "Pressed a key", "Pressing a key"),
    ("browser_back", "Went back a page", "Going back a page"),
    (
        "browser_forward",
        "Went forward a page",
        "Going forward a page",
    ),
    (
        "browser_text",
        "Read the page's text",
        "Reading the page's text",
    ),
    (
        "browser_screenshot",
        "Took a screenshot",
        "Taking a screenshot",
    ),
    (
        "browser_save_pdf",
        "Saved the page as a PDF",
        "Saving the page as a PDF",
    ),
    (
        "browser_wait",
        "Waited for the page",
        "Waiting for the page",
    ),
    (
        "browser_hover",
        "Hovered over the page",
        "Hovering over the page",
    ),
    ("browser_new_tab", "Opened a tab", "Opening a tab"),
    ("browser_upload", "Attached files", "Attaching files"),
    ("browser_tabs", "Managed the tabs", "Managing the tabs"),
    ("browser_sign_in", "Signed in", "Signing in"),
    (
        "browser_fill",
        "Filled a field from the vault",
        "Filling a field from the vault",
    ),
    ("browser_ask_code", "Asked for a code", "Asking for a code"),
    (
        "browser_ask_user",
        "Asked for help on the page",
        "Asking for help on the page",
    ),
    (
        "show_in_panel",
        "Showed it in the panel",
        "Showing it in the panel",
    ),
    ("show_file", "Showed a file", "Showing a file"),
    ("mark_final", "Marked a file final", "Marking a file final"),
    ("show_widget", "Showed a widget", "Showing a widget"),
    ("list_agents", "Listed the agents", "Listing the agents"),
    (
        "read_conversation",
        "Read a conversation",
        "Reading a conversation",
    ),
    ("send_message", "Sent a message", "Sending a message"),
    (
        "schedule_wake_up",
        "Scheduled a wake-up",
        "Scheduling a wake-up",
    ),
    ("watch_for_change", "Set up a watch", "Setting up a watch"),
    (
        "list_wake_ups",
        "Listed the wake-ups",
        "Listing the wake-ups",
    ),
    (
        "cancel_wake_up",
        "Canceled a wake-up",
        "Canceling a wake-up",
    ),
    ("set_job", "Updated its job", "Updating its job"),
    ("start_session", "Started a session", "Starting a session"),
    (
        "list_sessions",
        "Listed its sessions",
        "Listing its sessions",
    ),
    ("stop_session", "Stopped a session", "Stopping a session"),
    (
        "compact_session",
        "Summarized a session",
        "Summarizing a session",
    ),
    ("answer_ask", "Answered a session", "Answering a session"),
    ("list_models", "Listed the models", "Listing the models"),
    ("make_project", "Made a project", "Making a project"),
    (
        "list_projects",
        "Listed the projects",
        "Listing the projects",
    ),
    (
        "plan_limits",
        "Checked the plan's limits",
        "Checking the plan's limits",
    ),
    (
        "use_limit_reset",
        "Used a limit reset",
        "Using a limit reset",
    ),
    (
        "notify_user",
        "Sent you a notification",
        "Sending you a notification",
    ),
    (
        "queue_list",
        "Listed the Work Queue",
        "Listing the Work Queue",
    ),
    (
        "queue_show",
        "Read a Work Queue item",
        "Reading a Work Queue item",
    ),
    (
        "queue_add",
        "Added to the Work Queue",
        "Adding to the Work Queue",
    ),
    (
        "queue_update",
        "Updated a Work Queue item",
        "Updating a Work Queue item",
    ),
    (
        "queue_hand",
        "Handed off a Work Queue item",
        "Handing off a Work Queue item",
    ),
    (
        "queue_remove",
        "Removed a Work Queue item",
        "Removing a Work Queue item",
    ),
    (
        "queue_labels",
        "Managed the Work Queue's labels",
        "Managing the Work Queue's labels",
    ),
    (
        "queue_import",
        "Imported items into the Work Queue",
        "Importing items into the Work Queue",
    ),
    (
        "vault_items",
        "Listed the vault's items",
        "Listing the vault's items",
    ),
    (
        "vault_ask",
        "Asked for a vault item",
        "Asking for a vault item",
    ),
    (
        "vault_run",
        "Ran a command with vault values",
        "Running a command with vault values",
    ),
    (
        "sandbox_ask",
        "Asked to run a command outside the sandbox",
        "Asking to run a command outside the sandbox",
    ),
];

pub(super) struct Target {
    pub(super) field: &'static str,
    pub(super) said: &'static str,
    pub(super) doing: &'static str,
    pub(super) quoted: bool,
}

pub(super) fn target(name: &str) -> Option<Target> {
    let (field, said, doing, quoted) = match name {
        "read_conversation" => (
            "of",
            "Read {}’s conversation",
            "Reading {}’s conversation",
            false,
        ),
        "send_message" => (
            "to",
            "Sent a message to {}",
            "Sending a message to {}",
            false,
        ),
        "make_project" => (
            "name",
            "Made the project {}",
            "Making the project {}",
            false,
        ),
        "queue_add" => (
            "title",
            "Added {} to the Work Queue",
            "Adding {} to the Work Queue",
            true,
        ),
        _ => return None,
    };
    Some(Target {
        field,
        said,
        doing,
        quoted,
    })
}

pub(super) const MCP: &[(&str, &str)] = &[
    ("linear", "Linear"),
    ("sentry", "Sentry"),
    ("notion", "Notion"),
    ("atlassian", "Jira and Confluence"),
    ("stripe", "Stripe"),
    ("supabase", "Supabase"),
    ("neon", "Neon"),
    ("context7", "Context7"),
    ("cloudflare-docs", "Cloudflare Docs"),
    ("huggingface", "Hugging Face"),
    ("chrome-devtools", "Chrome DevTools"),
    ("apple-calendar", "Apple Calendar and Reminders"),
    ("microsoft-365", "Outlook and Microsoft 365"),
    ("fastmail", "Fastmail"),
    ("caldav", "Other Calendars"),
    ("gmail", "Gmail"),
    ("google-calendar", "Google Calendar"),
    ("slack", "Slack"),
    ("github", "GitHub"),
    ("gitlab", "GitLab"),
    ("todoist", "Todoist"),
    ("calendly", "Calendly"),
    ("figma", "Figma"),
    ("asana", "Asana"),
    ("zoom", "Zoom"),
];
