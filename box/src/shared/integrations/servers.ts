// Well-known MCP servers, each from the company behind it as listed in the official MCP Registry
// (registry.modelcontextprotocol.io), to add in one click (Settings › MCP Servers; PROTOCOL.md,
// `kuvo/mcp/catalog`). Servers that need an account sign in through the engine when they're first
// used. A box's release carries them as `bin/mcp-catalog.json` (`catalogs.ts`).

import { APART, LAUNCHER } from "./table";

/** One of them: `id` is its name in the engine's settings, which its tools are named after;
 * `access`, how it's reached (an account, none, or run on the computer); `settings`, the engine's
 * settings for it. */
export interface Server {
  id: string;
  title: string;
  summary: string;
  access: "signIn" | "open" | "local";
  settings: Record<string, unknown>;
}

const remote = (url: string) => ({ type: "remote", url });
/** Run through `kuvo-npx`, pinned, with `environment`. */
const local = (command: string[], environment: Record<string, string>) => ({ type: "local", command: [LAUNCHER, ...command], environment });

/** Servers that use a Mac's own apps (its Chrome), left out of machines' settings. */
export const MAC_ONLY_SERVERS = ["chrome-devtools"];

export const SERVERS: Server[] = [
  { id: "linear", title: "Linear", summary: "Issues, projects and cycles", access: "signIn", settings: remote("https://mcp.linear.app/mcp") },
  { id: "sentry", title: "Sentry", summary: "Errors and performance problems in your apps", access: "signIn", settings: remote("https://mcp.sentry.dev/mcp") },
  { id: "notion", title: "Notion", summary: "Pages and databases in your workspace", access: "signIn", settings: remote("https://mcp.notion.com/mcp") },
  { id: "atlassian", title: "Atlassian", summary: "Jira issues and Confluence pages", access: "signIn", settings: remote("https://mcp.atlassian.com/v2/mcp") },
  { id: "stripe", title: "Stripe", summary: "Payments, customers and Stripe's docs", access: "signIn", settings: remote("https://mcp.stripe.com") },
  { id: "supabase", title: "Supabase", summary: "Databases, auth and storage in your projects", access: "signIn", settings: remote("https://mcp.supabase.com/mcp") },
  { id: "neon", title: "Neon", summary: "Postgres databases and branches", access: "signIn", settings: remote("https://mcp.neon.tech/mcp") },
  { id: "context7", title: "Context7", summary: "Current docs and examples for libraries", access: "open", settings: remote("https://mcp.context7.com/mcp") },
  { id: "cloudflare-docs", title: "Cloudflare Docs", summary: "Cloudflare's documentation", access: "open", settings: remote("https://docs.mcp.cloudflare.com/mcp") },
  { id: "huggingface", title: "Hugging Face", summary: "Models, datasets, Spaces and papers", access: "open", settings: remote("https://huggingface.co/mcp") },
  // Chrome DevTools' server, with a profile of its own for each copy, in the temporary folder and
  // gone when it closes (not one in your home folder, which a second project's copy couldn't
  // open), and nothing sent to Google about its use. Its package got apart from the project (its
  // .npmrc, .env), run in it (where a screenshot it saves goes); no update checks or usage
  // statistics, which it keeps in your home folder.
  { id: "chrome-devtools", title: "Chrome DevTools", summary: "Inspect and debug pages in Google Chrome", access: "local",
    settings: local(["chrome-devtools-mcp@1.12.0", "--isolated", "--no-usage-statistics"], {
      [APART]: "fetch",
      CHROME_DEVTOOLS_MCP_NO_UPDATE_CHECKS: "1",
      CHROME_DEVTOOLS_MCP_NO_USAGE_STATISTICS: "1",
    }) },
];
