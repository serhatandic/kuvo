// The services an agent can work with (PROTOCOL.md, "Integrations"): your mail and calendar, chats,
// socials, code and work tools. Each the best way it can be: its maker's own MCP server where
// there is one (you sign in once); else a well-kept open-source server with a key you bring; else
// the site itself, in the box's browser, signed in as you. This is the one table: a box's release
// carries it as the catalogs every box reads (`catalogs.ts`, `bin/integrations.json`,
// `bin/integrations-mac.json` and, with `servers.ts`, `bin/mcp-catalog.json`).
//
// Checked 2026-10-16: the official servers with their makers' docs; the open-source ones on npm,
// pinned. Keep them current with the other upstreams (docs/upstreams.md).

/** A key a service takes: the environment variable its server reads (for an app, CLIENT_ID and
 * CLIENT_SECRET), and how Settings asks for it. */
export interface Key {
  name: string;
  label: string;
  prompt: string;
}

/** How a service is added. */
export type Way =
  /** Its maker's MCP server at `url`, where you sign in with your account the first time. */
  | { way: "official"; url: string }
  /** Its maker's server, with a token you bring, sent as `Authorization: Bearer`. */
  | { way: "token"; url: string; key: Key; help: string }
  /** Its maker's server, which takes sign-ins only through an app registered with it: you bring
   * your app's client ID and secret, then sign in. */
  | { way: "app"; url: string; help: string }
  /** An open-source MCP server (an npm package, pinned to a version), run with `kuvo-npx`, with
   * the keys you bring and settings of its own; `macOnly` for one that uses a Mac's own apps. */
  | { way: "package"; command: string[]; keys: Key[]; help?: string; by: string; environment?: Record<string, string>; macOnly?: boolean }
  /** No server worth trusting: the agent uses the site in the browser, signed in as you. */
  | { way: "browser" };

export const KINDS = { mail: "Mail and Calendar", chat: "Chat", social: "Social", work: "Code and Work" } as const;

export type Service = {
  id: string;
  title: string;
  /** What the agent can do with it. */
  summary: string;
  kind: keyof typeof KINDS;
  /** Its picture, by its SF Symbols name. */
  symbol: string;
  /** Its site, for the browser, and where to sign in there. */
  site: string;
} & Way;

/** Where the engine's own sign-in comes back for a server that signs in through your app. */
export const REDIRECT = "http://127.0.0.1:19876/mcp/oauth/callback";

/** What tells `kuvo-npx` to run a server apart from the project (`fetch`: only getting its package). */
export const APART = "KUVO_NPX_APART";

/** The command that runs an npm package's server (`bin/kuvo-npx`, `box/src/keeper/kuvo-npx`). */
export const LAUNCHER = "kuvo-npx";

/** The cookie a site keeps while you're signed in to it, for those the agent uses on their site:
 * how a box tells whether its browser is signed in there. (The others keep their sign-in where it
 * can't be read from outside the page.) */
export const SIGN_IN_COOKIES: Record<string, { domain: string; name: string }> = {
  x: { domain: "x.com", name: "auth_token" },
  linkedin: { domain: "linkedin.com", name: "li_at" },
  reddit: { domain: "reddit.com", name: "reddit_session" },
};

/** Integrations whose servers keep their sign-in in files of their own, in their folder, by the
 * files' names, the sign-in itself first. They go together: Outlook's is encrypted with the key in
 * `.cache-key`, which each side makes for itself. */
export const SIGN_IN_FILES: Record<string, string[]> = {
  "microsoft-365": ["token-cache.json", "selected-account.json", ".cache-key"],
};

const appKeys: Key[] = [
  { name: "CLIENT_ID", label: "Client ID", prompt: "From your app" },
  { name: "CLIENT_SECRET", label: "Client secret", prompt: "From your app" },
];

/** The keys a service takes, if any. */
export function keysOf(service: Service): Key[] {
  switch (service.way) {
    case "token": return [service.key];
    case "app": return appKeys;
    case "package": return service.keys;
    default: return [];
  }
}

export const SERVICES: Service[] = [
  // Mail and calendar
  { id: "apple-calendar", title: "Apple Calendar and Reminders", summary: "Events and reminders in your Mac's Calendar and Reminders, with every account you have there", kind: "mail", symbol: "calendar", site: "https://www.icloud.com/calendar",
    way: "package", command: ["mcp-server-apple-events@1.6.2"], keys: [], by: "FradSer's mcp-server-apple-events", macOnly: true },
  { id: "microsoft-365", title: "Outlook and Microsoft 365", summary: "Mail, calendar, contacts and files; you sign in with a code the first time", kind: "mail", symbol: "envelope", site: "https://outlook.live.com",
    // --discovery: its own way of keeping its many tools (well over 100k tokens) out of every
    // request; the agent searches them and runs the one it needs.
    way: "package", command: ["@softeria/ms-365-mcp-server@0.175.0", "--discovery"], keys: [], by: "Softeria's ms-365-mcp-server", environment: {
      // Its sign-in, chosen account and logs in a folder of its own, not the home folder or the keychain.
      MS365_MCP_TOKEN_CACHE_PATH: "{env:KUVO_INTEGRATIONS}/microsoft-365/token-cache.json",
      MS365_MCP_SELECTED_ACCOUNT_PATH: "{env:KUVO_INTEGRATIONS}/microsoft-365/selected-account.json",
      MS365_MCP_LOG_DIR: "{env:KUVO_INTEGRATIONS}/microsoft-365/logs",
      MS365_MCP_USE_KEYTAR: "0",
    } },
  { id: "fastmail", title: "Fastmail", summary: "Mail, contacts and calendars", kind: "mail", symbol: "envelope.badge", site: "https://app.fastmail.com",
    way: "official", url: "https://api.fastmail.com/mcp" },
  { id: "caldav", title: "Other Calendars", summary: "iCloud, Nextcloud and other CalDAV calendars", kind: "mail", symbol: "calendar.badge.clock", site: "https://www.icloud.com/calendar",
    way: "package", command: ["caldav-mcp@0.10.3"], keys: [
      { name: "CALDAV_BASE_URL", label: "Server", prompt: "https://caldav.icloud.com" },
      { name: "CALDAV_USERNAME", label: "Username", prompt: "you@icloud.com" },
      { name: "CALDAV_PASSWORD", label: "Password", prompt: "An app-specific password" },
    ], help: "https://support.apple.com/en-us/102654", by: "dominik1001's caldav-mcp" },
  { id: "gmail", title: "Gmail", summary: "Google's own server, in preview: with an app in your Google Cloud project", kind: "mail", symbol: "envelope.open", site: "https://mail.google.com",
    way: "app", url: "https://gmailmcp.googleapis.com/mcp/v1", help: "https://developers.google.com/workspace/guides/configure-mcp-servers" },
  { id: "google-calendar", title: "Google Calendar", summary: "Google's own server, in preview: with an app in your Google Cloud project", kind: "mail", symbol: "calendar.circle", site: "https://calendar.google.com",
    way: "app", url: "https://calendarmcp.googleapis.com/mcp/v1", help: "https://developers.google.com/workspace/guides/configure-mcp-servers" },
  // Chat
  { id: "slack", title: "Slack", summary: "Channels, messages and search, through an app you add to your workspace", kind: "chat", symbol: "bubble.left.and.bubble.right", site: "https://app.slack.com",
    way: "app", url: "https://mcp.slack.com/mcp", help: "https://docs.slack.dev/ai/mcp-server/" },
  { id: "discord", title: "Discord", summary: "Your servers and messages", kind: "chat", symbol: "bubble.left.and.bubble.right", site: "https://discord.com/app", way: "browser" },
  { id: "telegram", title: "Telegram", summary: "Your chats", kind: "chat", symbol: "paperplane", site: "https://web.telegram.org", way: "browser" },
  { id: "whatsapp", title: "WhatsApp", summary: "Your chats", kind: "chat", symbol: "phone.bubble", site: "https://web.whatsapp.com", way: "browser" },
  // Social
  { id: "x", title: "X", summary: "Posts, replies and messages", kind: "social", symbol: "at", site: "https://x.com", way: "browser" },
  { id: "linkedin", title: "LinkedIn", summary: "Your feed, posts and messages", kind: "social", symbol: "person.2", site: "https://www.linkedin.com", way: "browser" },
  { id: "reddit", title: "Reddit", summary: "Posts and comments", kind: "social", symbol: "text.bubble", site: "https://www.reddit.com", way: "browser" },
  { id: "bluesky", title: "Bluesky", summary: "Posts and replies", kind: "social", symbol: "cloud", site: "https://bsky.app", way: "browser" },
  { id: "mastodon", title: "Mastodon", summary: "Posts and replies", kind: "social", symbol: "megaphone", site: "https://mastodon.social", way: "browser" },
  // Code and work
  { id: "github", title: "GitHub", summary: "Repositories, issues, pull requests and Actions", kind: "work", symbol: "chevron.left.forwardslash.chevron.right", site: "https://github.com",
    way: "token", url: "https://api.githubcopilot.com/mcp/", key: { name: "TOKEN", label: "Personal access token", prompt: "github_pat_…" }, help: "https://github.com/settings/personal-access-tokens/new" },
  { id: "gitlab", title: "GitLab", summary: "Projects, issues and merge requests", kind: "work", symbol: "arrow.triangle.branch", site: "https://gitlab.com",
    way: "official", url: "https://gitlab.com/api/v4/mcp" },
  { id: "linear", title: "Linear", summary: "Issues, projects and cycles", kind: "work", symbol: "checklist", site: "https://linear.app",
    way: "official", url: "https://mcp.linear.app/mcp" },
  { id: "notion", title: "Notion", summary: "Pages and databases", kind: "work", symbol: "doc.text", site: "https://www.notion.so",
    way: "official", url: "https://mcp.notion.com/mcp" },
  { id: "atlassian", title: "Jira and Confluence", summary: "Issues and pages at Atlassian", kind: "work", symbol: "square.stack.3d.up", site: "https://www.atlassian.com",
    way: "official", url: "https://mcp.atlassian.com/v2/mcp" },
  { id: "todoist", title: "Todoist", summary: "Tasks and projects", kind: "work", symbol: "checkmark.circle", site: "https://app.todoist.com",
    way: "package", command: ["@doist/todoist-mcp@13.7.1"], keys: [{ name: "TODOIST_API_KEY", label: "API token", prompt: "From Settings › Integrations › Developer" }], help: "https://app.todoist.com/app/settings/integrations/developer", by: "Doist's own todoist-mcp" },
  { id: "calendly", title: "Calendly", summary: "Your event types, bookings and availability", kind: "work", symbol: "calendar.badge.plus", site: "https://calendly.com/app",
    way: "official", url: "https://mcp.calendly.com" },
  { id: "figma", title: "Figma", summary: "Files and frames, to build designs from", kind: "work", symbol: "paintbrush", site: "https://www.figma.com",
    way: "official", url: "https://mcp.figma.com/mcp" },
  { id: "asana", title: "Asana", summary: "Tasks and projects, through an app you register", kind: "work", symbol: "circle.grid.3x3", site: "https://app.asana.com",
    way: "app", url: "https://mcp.asana.com/v2/mcp", help: "https://developers.asana.com/docs/using-asanas-mcp-server" },
  { id: "zoom", title: "Zoom", summary: "Meetings, recordings and chat, through an app you register", kind: "work", symbol: "video", site: "https://zoom.us/meeting",
    way: "app", url: "https://mcp.zoom.us/mcp/zoom/streamable", help: "https://developers.zoom.us/docs/mcp/servers/connect-to-zoom-mcp-servers/" },
];
