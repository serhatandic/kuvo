//! What the stand-in box says of its accounts and settings, and how it signs in: the engine's plans
//! and providers and the coding agents' accounts (`…/api/integration`), the ways in the box offers
//! for each, with what each is, whether Accounts lists it, its mark, how it stands, its line and
//! what can be done with it (`kuvo/sign-in/accounts`,
//! `…/methods`, shaped as `Vectors/sign-in.json` records them), sign-ins started through the box
//! (`kuvo/sign-in/start`, with a device code apart) that go on until a test says how they end (or, in the app's `--stand-in`, finish after a
//! few asks), keys, installs, Disconnect and Sign Out; and this computer's settings (`kuvo/config`,
//! `kuvo/approval`, `kuvo/sandbox`, `kuvo/sandbox/allowed`, `kuvo/browser/settings`, the usage
//! settings). All made up: no key is real, and every page is at `.invalid`, which never resolves.

use std::collections::BTreeMap;

pub mod agents;
pub mod mcp;
mod pages;
pub mod vault;

use serde_json::{Map, Value, json};

/// How a sign-in under way stands.
#[derive(Clone, Debug)]
struct Attempt {
    agents: bool,
    account: String,
    status: Value,
    asked: u32,
}

/// The stand-in's accounts and settings.
#[derive(Clone, Debug)]
pub struct Settings {
    engine: Vec<Value>,
    agents: Vec<Value>,
    ways: BTreeMap<String, Value>,
    ended: Vec<String>,
    /// What a test has the box say of an account, in place of what it would.
    said: BTreeMap<String, Value>,
    attempts: BTreeMap<String, Attempt>,
    next: u32,
    /// The bridge isn't running (`503`).
    pub agents_down: bool,
    /// Sign-ins finish by themselves after a few asks (the app's `--stand-in`).
    pub finishes: bool,
    config: Value,
    /// This computer's modes (`kuvo/approval`): Full Access confirmed, the default, Auto held.
    pub approval: Value,
    sandbox: Value,
    allowed: Vec<Value>,
    browser: Value,
    usage: Value,
    usage_version: u32,
    /// Your time zone, once picked (`kuvo/settings/clock`).
    zone: Option<String>,
    /// Your own rule for what the agent does without asking, by kind (`when-<kind>`).
    when: BTreeMap<String, String>,
    /// Sections a test added to `kuvo/settings`: each one's line there, and its page.
    more_sections: Vec<(Value, Value)>,
    /// Its sections can't be read now (`503`).
    pub pages_down: bool,
    /// How this computer stands (`kuvo/settings/computer`).
    pub computer: Computer,
    /// Its MCP servers (`kuvo/settings/mcp`), listed once it's on.
    pub mcp: mcp::Mcp,
    /// Its Agents section (`kuvo/settings/agents`), listed once it's on.
    pub agent_settings: agents::Agents,
    /// Its Vault (`kuvo/settings/vault`), listed once it's on.
    pub vault: vault::Vault,
}

/// How the stand-in's computer stands, as its own settings page says it. All made up.
#[derive(Clone, Debug)]
pub struct Computer {
    /// "Let my devices connect".
    pub devices: bool,
    /// Whether Kuvo Helper is on in Login Items.
    pub at_login: bool,
    pub version: String,
    /// A newer version it knows of.
    pub ready: Option<String>,
    pub updating: bool,
    /// Whether agents are at work.
    pub working: bool,
    /// How many times Restart and its update's button were pressed.
    pub restarted: u32,
    pub asked_to_update: u32,
    /// What macOS allows its helper (the stand-in's own, as `kuvo/box/permissions` says it).
    pub permissions: super::permissions::Permissions,
}

impl Default for Computer {
    fn default() -> Computer {
        Computer { devices: true, at_login: true, version: "1.0.0".into(), ready: None, updating: false, working: false, restarted: 0, asked_to_update: 0, permissions: super::permissions::Permissions::default() }
    }
}

/// Git, Worktrees, Storage and Archived Sessions as a Mac's box describes them to the app beside it
/// (`box/src/link/areas/settings-pages.ts`), with made-up values; Storage as it's first measured
/// where `measuring`.
pub fn described_pages(measuring: bool) -> Vec<(Value, Value)> {
    let section = |id: &str, title: &str, group: &str, icon: &str| json!({ "id": id, "title": title, "group": group, "icon": icon, "line": "", "keywords": [] });
    let git = json!({ "id": "git", "title": "Git", "groups": [
        { "id": "branches", "rows": [{ "id": "branch-prefix", "title": "Branch prefix", "line": "How a session’s own branch in a worktree starts.", "placeholder": "None", "kind": "text", "value": "kuvo/", "can": true }] },
        { "id": "commit-messages", "title": "Commit Instructions", "note": "Added when Suggest a Message in Changes asks the model for one.", "rows": [{ "id": "commit-instructions", "title": "Commit instructions", "lines": { "min": 5, "max": 12 }, "placeholder": "For example: Start with the area changed, like “ui: …”, and never mention tickets.", "kind": "text", "value": "", "can": true }] },
        { "id": "pull-requests", "title": "Pull Request Instructions", "note": "Added when Create Pull Request writes the title and description.", "rows": [{ "id": "pull-request-instructions", "title": "Pull request instructions", "lines": { "min": 5, "max": 12 }, "placeholder": "For example: Add a Testing section, and link the issue if the branch names one.", "kind": "text", "value": "Add a Testing section.\nLink the issue when the branch names one.", "can": true }] },
    ] });
    let folder_words = |clear: &str, message: &str| json!({ "choose": "Choose a Folder…", "clear": clear, "prompt": "Choose", "message": message });
    let tree = |branch: &str, project: &str, sessions: u32| json!({ "id": format!("tree-/Users/sam/.kuvo-code/data/opencode/worktree/{branch}"), "title": branch, "line": format!("{project} · {sessions} {}", if sessions == 1 { "session" } else { "sessions" }), "kind": "actions", "actions": [{ "id": "remove", "title": "Remove…", "style": "destructive", "confirm": { "title": format!("Remove the worktree for {branch}?"), "body": "Its folder is deleted. Git sees nothing in it that isn’t committed. Its sessions stay, without a folder to run in.", "button": "Remove" } }] });
    let worktrees = json!({ "id": "worktrees", "title": "Worktrees", "groups": [
        { "id": "folders", "rows": [
            { "id": "worktrees-folder", "title": "Worktree folder", "line": "Each session’s worktree is a folder in it. Leave it empty for the default.", "kind": "folder", "value": "", "placeholder": "~/.kuvo-code/data/opencode/worktree", "folder": folder_words("Default Folder", "Choose where new worktrees go. Each one is a folder in it."), "can": true },
            { "id": "projects-folder", "title": "New projects folder", "line": "Where projects your bots make go. A bot asks you first, and Kuvo never deletes them.", "kind": "folder", "value": "/Users/sam/Projects", "placeholder": "None", "folder": folder_words("None", "Choose where new projects go. Each one is a folder in it."), "can": true },
        ] },
        { "id": "session-worktrees", "title": "Sessions’ Worktrees", "note": "Removing one deletes its folder and anything in it that isn’t committed. Its branch stays if it isn’t merged yet.", "rows": [
            { "id": "worktrees", "title": "Sessions’ Worktrees", "kind": "list", "empty": "None yet. Choose Worktree in a new session’s composer to give it one.", "items": [tree("kuvo/settings-look", "kuvo-ai", 2), tree("kuvo/fix-sync", "api", 1), tree("kuvo/onboarding", "kuvo-web", 1)] },
        ] },
    ] });
    let size = |bytes: u64| -> Value { let unit = if bytes >= 1_000_000_000 { ("GB", 1_000_000_000) } else { ("MB", 1_000_000) }; json!({ "name": unit.0, "factor": unit.1, "decimals": 1, "rounding": "nearest" }) };
    let part = |id: &str, title: &str, value: u64, words: &str| json!({ "id": id, "title": title, "value": value, "words": words });
    let mut other = part("other", "Other", 1_840_000_000, "1.8 GB");
    other["parts"] = json!([
        { "title": "The engine", "value": 820_000_000_u64, "words": "820 MB" }, { "title": "Coding agents", "value": 610_000_000_u64, "words": "610 MB" },
        { "title": "Tools", "value": 240_000_000_u64, "words": "240 MB" }, { "title": "Deleted, kept a while", "value": 130_000_000_u64, "words": "130 MB" }, { "title": "Caches", "value": 40_000_000_u64, "words": "40 MB" },
    ]);
    let used = if measuring {
        json!({ "id": "used", "title": "Measuring what Kuvo keeps…", "kind": "meter", "busy": "Measuring…", "actions": [{ "id": "refresh", "title": "Refresh" }] })
    } else {
        json!({ "id": "used", "title": "9.4 GB used by Kuvo", "kind": "meter", "actions": [{ "id": "refresh", "title": "Refresh" }], "parts": [
            part("sessions", "Sessions", 1_210_000_000, "1.2 GB"), part("worktrees", "Worktrees", 5_630_000_000, "5.6 GB"), part("made", "What agents made", 420_000_000, "420 MB"), part("browser", "Browser", 300_000_000, "300 MB"), other,
        ] })
    };
    let suggestions: Vec<Value> = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0].iter().map(|gb: &f64| json!({ "value": (gb * 1e9) as u64, "title": format!("{gb} GB") })).collect();
    let storage = json!({ "id": "storage", "title": "Storage", "groups": [
        { "id": "computer", "title": "This Mac", "note": "What Kuvo keeps on this Mac.", "rows": [used] },
        { "id": "folders", "title": "Storage by Folder", "rows": [
            { "id": "home-folder", "title": "Kuvo’s folder", "line": "~/.kuvo-code", "kind": "actions", "value": 9_400_000_000_u64, "unit": size(9_400_000_000), "actions": [{ "id": "show", "title": "Show in Finder", "opens": "file:///Users/sam/.kuvo-code" }] },
        ] },
        { "id": "agents-made", "title": "What Agents Made", "rows": [
            { "id": "copies", "title": "Copies of what agents made", "line": "Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps.", "kind": "status", "value": 420_000_000_u64, "unit": size(420_000_000) },
            { "id": "older-budget", "title": "Older versions keep up to", "line": "Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay.", "kind": "number", "value": 1_000_000_000_u64, "unit": { "name": "GB", "factor": 1_000_000_000, "decimals": 2, "rounding": "nearest" }, "min": 0, "can": true,
              "suggestions": suggestions,
              "confirmBelow": { "value": 805_306_368_u64, "confirm": { "title": "Keep less of older versions?", "body": "Older versions take 805.3 MB now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.", "button": "Keep up to" } } },
            { "id": "older-copies", "title": "Older versions", "line": "805.3 MB now.", "kind": "actions", "value": 805_306_368_u64, "actions": [{ "id": "clear", "title": "Clear…", "style": "destructive", "confirm": { "title": "Clear older versions?", "body": "Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.", "button": "Clear" } }] },
        ] },
        { "id": "manage", "rows": [
            { "id": "manage-worktrees", "title": "Worktrees", "line": "Sessions’ own copies of a project, and where they go.", "kind": "open", "section": "worktrees" },
            { "id": "manage-archived", "title": "Archived Sessions", "line": "Bring sessions back, or delete them for good.", "kind": "open", "section": "archived" },
        ] },
    ] });
    let archived = json!({ "id": "archived", "title": "Archived Sessions", "groups": [{ "id": "project-kuvo", "title": "Kuvo Code", "rows": [{ "id": "archived-kuvo", "title": "Kuvo Code", "kind": "list", "search": { "placeholder": "Search archived sessions", "empty": "No archived session matches “{query}”." },
        "items": [{ "id": "session-1", "title": "Fix settings", "line": "Nov 14, 2026, 4:12 PM", "kind": "actions", "actions": [{ "id": "unarchive", "title": "Unarchive" }, { "id": "delete", "title": "Delete…", "style": "destructive" }] }] }] }] });
    vec![
        (section("git", "Git", "Projects", "branch"), git),
        (section("worktrees", "Worktrees", "Projects", "folder"), worktrees),
        (section("storage", "Storage", "System", "drive"), storage),
        (section("archived", "Archived Sessions", "System", "archive"), archived),
    ]
}

/// A section with a row of every kind a box may describe (PROTOCOL.md, "A box's settings"), for
/// the kinds its own pages don't have yet: its line in `kuvo/settings`, and its page.
pub fn every_kind() -> (Value, Value) {
    let section = json!({ "id": "samples", "title": "Every Kind", "group": "System", "icon": "sliders", "line": "A row of each kind Kuvo may describe.", "keywords": ["proxy"] });
    let page = json!({ "id": "samples", "title": "Every Kind", "line": "A row of each kind Kuvo may describe, drawn from its description alone.", "groups": [
        { "id": "fields", "title": "Fields", "note": "Saved with Return, or as you leave the field.", "rows": [
            { "id": "proxy", "title": "Proxy", "line": "For what agents fetch.", "kind": "text", "value": "", "placeholder": "http://proxy.example:8080", "can": true },
            { "id": "keep", "title": "Keep sessions for", "line": "Days, before they’re archived.", "kind": "number", "value": 30, "min": 1, "max": 365, "can": true },
            { "id": "projects", "title": "Projects folder", "line": "Where new projects are made.", "kind": "folder", "value": "/home/sam/projects", "can": true },
            { "id": "token", "title": "Registry token", "line": "For private packages.", "kind": "secret", "set": true, "can": true },
            { "id": "key", "title": "Signing key", "kind": "secret", "set": false, "placeholder": "Paste a key", "can": true },
        ] },
        { "id": "more", "title": "More", "rows": [
            { "id": "reads", "title": "This month", "line": "What its plan allows.", "kind": "meter", "windows": [{ "title": "Requests", "used": 0.42, "resets": 1_794_900_000_000.0_f64 }] },
            { "id": "rules", "title": "Rules", "line": "For particular commands.", "kind": "open", "section": "permissions" },
            { "id": "docs", "title": "How it works", "kind": "open", "address": "https://www.kuvo.run/code/docs/settings/" },
            { "id": "pair", "title": "Pair a phone", "line": "Scan a code with Kuvo Code for iPhone.", "kind": "steps", "start": { "id": "start", "title": "Pair…" } },
            { "id": "background", "title": "Keep working with the app closed", "kind": "custom", "name": "background-work", "can": { "why": "Turn it on in Kuvo Code on that computer." } },
            { "id": "later", "title": "Something newer", "line": "From a newer version of Kuvo.", "kind": "hologram" },
            { "id": "cache", "title": "Cache", "line": "1.2 GB of builds.", "kind": "actions", "actions": [{ "id": "clear", "title": "Clear…", "style": "destructive", "confirm": { "title": "Clear the cache?", "body": "The next build takes longer.", "button": "Clear" } }, { "id": "show", "title": "Show", "can": { "why": "Only on that computer itself." } }] },
        ] },
        { "id": "hosts", "title": "Hosts", "rows": [
            { "id": "hosts", "title": "Trusted hosts", "line": "Agents fetch from these without asking.", "kind": "list", "add": { "id": "add", "title": "Add…", "can": { "why": "Adding one isn’t offered from here yet." } }, "empty": "None yet.", "items": [] },
        ] },
    ] });
    (section, page)
}

/// What a sign-in under way comes to, when a test says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Complete,
    Failed,
    Expired,
}

fn integration(id: &str, name: &str, connections: Value, metadata: Value) -> Value {
    json!({ "id": id, "name": name, "methods": [], "connections": connections, "metadata": metadata })
}

fn ways(methods: Value, environment: Value, hint: &str) -> Value {
    json!({ "methods": methods, "environment": environment, "hint": hint })
}

fn signed_in(label: &str, method: &str) -> Value {
    json!([{ "type": "credential", "id": format!("cred_{}", label.len()), "method": method, "label": label }])
}

impl Default for Settings {
    fn default() -> Settings {
        let engine = vec![
            integration("github-copilot", "GitHub Copilot", signed_in("sam-codes", "oauth"), json!({})),
            integration("openrouter", "OpenRouter", signed_in("Team key", "key"), json!({})),
            integration("gitlab", "GitLab Duo", signed_in("sam", "oauth"), json!({})),
            integration("anthropic", "Anthropic", json!([]), json!({})),
            integration("openai", "OpenAI", json!([]), json!({})),
            integration("amazon-bedrock", "Amazon Bedrock", json!([]), json!({})),
            integration("deepseek", "DeepSeek", json!([]), json!({})),
            integration("groq", "Groq", json!([]), json!({})),
            integration("mistral", "Mistral", json!([]), json!({})),
            // Its page turns every sign-in down (`access_denied`), for what a failure shows.
            integration("poe", "Poe", json!([]), json!({})),
            // A web search service, an MCP server's sign-in and the engine's own way to Claude's
            // plan, which the engine lists too: the box says Accounts doesn't list them.
            integration("tavily", "Tavily", json!([]), json!({})),
            integration("linear", "Linear", json!([]), json!({ "source": "mcp" })),
            integration("claude-agent", "Claude Agent", json!([]), json!({})),
        ];
        let agents = vec![
            integration("claude-code", "Claude Code", signed_in("sam@example.com", "oauth"), json!({ "addedAccounts": true, "agent": "claude", "installed": true, "version": "2.4.1", "remoteSignIn": true })),
            integration("claude-code@w0rk1a2b", "Claude Code · Work", signed_in("sam@studio.example", "oauth"), json!({ "addedAccounts": true, "agent": "claude", "installed": true, "version": "2.4.1", "accountID": "w0rk1a2b", "accountLabel": "Work", "remoteSignIn": true })),
            integration("codex-cli", "Codex", signed_in("sam@example.com", "oauth"), json!({ "addedAccounts": true, "agent": "codex", "installed": true, "version": "0.98.0", "signsOut": true, "remoteSignIn": true })),
            integration("antigravity", "Antigravity", signed_in("sam@example.com", "oauth"), json!({ "addedAccounts": true, "agent": "antigravity", "installed": true, "signInEnded": true, "remoteSignIn": true })),
            integration("cursor", "Cursor", json!([]), json!({ "addedAccounts": true, "agent": "cursor", "installed": true, "version": "1.9.0", "remoteSignIn": true })),
            integration("grok-cli", "Grok", json!([]), json!({ "addedAccounts": true, "agent": "grok", "installed": false, "install": "npm install -g @stand-in/grok-cli", "remoteSignIn": true })),
        ];
        let mut all = BTreeMap::new();
        let browser_sign_in = json!([{ "type": "oauth", "id": "browser", "label": "Sign in" }]);
        for agent in ["claude-code", "claude-code@w0rk1a2b", "antigravity", "grok-cli"] {
            all.insert(format!("agents:{agent}"), ways(browser_sign_in.clone(), json!([]), "Sign in"));
        }
        for agent in ["codex-cli", "cursor"] {
            all.insert(format!("agents:{agent}"), ways(json!([{ "type": "oauth", "id": "browser", "label": "Sign in" }, { "type": "key", "label": "API key" }]), json!([]), "Sign in"));
        }
        let region = json!([{ "key": "region", "type": "text", "label": "Region", "default": "us-east-1" }]);
        all.insert("github-copilot".into(), ways(json!([{ "type": "oauth", "id": "device", "label": "Sign in with GitHub" }]), json!([]), "Sign in with GitHub"));
        all.insert("openrouter".into(), ways(json!([{ "type": "key", "label": "API key" }]), json!([]), "API key"));
        all.insert("gitlab".into(), ways(json!([{ "type": "oauth", "id": "browser", "label": "Login with GitLab" }]), json!([]), "Sign in with GitLab"));
        all.insert(
            "anthropic".into(),
            ways(json!([{ "type": "oauth", "id": "max", "label": "Claude Pro/Max" }, { "type": "oauth", "id": "console", "label": "Create an API Key" }, { "type": "key", "label": "Manually enter API Key" }, { "type": "oauth", "id": "legacy-device", "label": "Device code (legacy)", "legacy": true }]), json!(["ANTHROPIC_API_KEY"]), "Claude Pro/Max"),
        );
        all.insert("openai".into(), ways(json!([{ "type": "key", "label": "Use an API key" }]), json!(["OPENAI_API_KEY"]), "API key"));
        all.insert("amazon-bedrock".into(), ways(json!([{ "type": "external", "id": "aws", "label": "AWS credentials on this computer", "form": region }, { "type": "key", "label": "Bedrock API key", "form": region }]), json!(["AWS_ACCESS_KEY_ID", "AWS_PROFILE"]), "Connect"));
        all.insert("deepseek".into(), ways(json!([{ "type": "key", "label": "Use an API key" }]), json!(["DEEPSEEK_API_KEY"]), "API key"));
        all.insert("groq".into(), ways(json!([]), json!(["GROQ_API_KEY"]), "Connect"));
        all.insert("mistral".into(), ways(json!([{ "type": "key", "label": "Use an API key" }]), json!(["MISTRAL_API_KEY"]), "API key"));
        all.insert("poe".into(), ways(json!([{ "type": "oauth", "id": "device", "label": "Sign in" }]), json!([]), "Sign in"));
        all.insert("tavily".into(), ways(json!([{ "type": "key", "label": "API key" }]), json!([]), "API key"));
        Settings {
            engine,
            agents,
            ways: all,
            ended: vec!["gitlab".into()],
            said: BTreeMap::new(),
            attempts: BTreeMap::new(),
            next: 1,
            agents_down: false,
            finishes: false,
            config: json!({ "$schema": "https://opencode.ai/config.json", "model": "anthropic/claude-sonnet-5-5", "agents": { "title": { "model": "google/gemini-3-pro" } } }),
            approval: json!({ "fullAccess": false, "default": "auto", "auto": { "engine": {}, "agents": {} }, "otherComputers": false }),
            sandbox: json!({ "on": true, "able": true, "agents": true }),
            allowed: vec![
                json!({ "id": "alw_1", "project": "/Users/sam/Projects/storefront", "kind": "write", "path": "/Users/sam/Library/Caches/storefront-build", "at": 1_794_000_000_000.0_f64, "owner": { "kind": "project", "name": "storefront" } }),
                json!({ "id": "alw_2", "kind": "write", "path": "/Users/sam/Documents/Release Notes", "at": 1_793_500_000_000.0_f64, "owner": { "kind": "bot", "id": "bot_release", "name": "Release Captain" } }),
            ],
            browser: json!({ "searchEngine": "duckDuckGo", "searchEngines": [{ "id": "google", "title": "Google" }, { "id": "duckDuckGo", "title": "DuckDuckGo" }, { "id": "bing", "title": "Bing" }, { "id": "brave", "title": "Brave Search" }, { "id": "kagi", "title": "Kagi" }, { "id": "ecosia", "title": "Ecosia" }], "tidiesAgentTabs": true, "offersToSave": true, "never": ["bank.example", "intranet.example"] }),
            usage: json!({ "underTenPercent": true, "healthyToClose": false, "closeToRunningOut": true }),
            usage_version: 3,
            zone: None,
            when: BTreeMap::new(),
            more_sections: Vec::new(),
            pages_down: false,
            computer: Computer::default(),
            mcp: mcp::Mcp::default(),
            agent_settings: agents::Agents::default(),
            vault: vault::Vault::default(),
        }
    }
}

fn path_part(part: &str) -> String {
    super::percent_decode(part)
}

impl Settings {
    /// A new computer: no coding agent's program is installed and nothing is signed in.
    pub fn nothing_installed(&mut self) {
        for agent in &mut self.agents {
            agent["connections"] = json!([]);
            agent["metadata"]["installed"] = json!(false);
            agent["metadata"]["install"] = json!("npm install -g @stand-in/agent");
            if let Some(metadata) = agent["metadata"].as_object_mut() {
                metadata.remove("signInEnded");
                metadata.remove("version");
            }
        }
        // (An account you added goes with its sign-in.)
        self.agents.retain(|agent| !agent["id"].as_str().is_some_and(|id| id.contains('@')));
        for account in &mut self.engine {
            account["connections"] = json!([]);
        }
        self.ended.clear();
    }

    /// The plans that need you, as a box says them in `kuvo/setup/ready` (`needsYou`): those
    /// Accounts lists, not added beside an agent's own, whose sign-in ended or says something's
    /// wrong, the coding agents' first; none where a side can't say.
    pub fn needs_you(&mut self) -> Option<Vec<String>> {
        let mut needing = Vec::new();
        for agents in [true, false] {
            let query = move |name: &str| (agents && name == "agents").then(|| "1".to_owned());
            // (Accounts say nothing by the computer's name or where the app runs.)
            let (status, said) = self.answer("GET", "kuvo/sign-in/accounts", &query, &Value::Null, super::NAME, true)?;
            if status != 200 {
                return None;
            }
            for account in said["accounts"].as_array().into_iter().flatten() {
                if account["listed"] == true && account["plan"] == true && account["added"] != true && (account["state"] == "ended" || account["state"] == "problem") {
                    needing.extend(account["id"].as_str().map(str::to_owned));
                }
            }
        }
        Some(needing)
    }

    /// Every sign-in there is works: none has ended.
    pub fn all_well(&mut self) {
        for agent in &mut self.agents {
            if let Some(metadata) = agent["metadata"].as_object_mut() {
                metadata.remove("signInEnded");
            }
        }
        self.ended.clear();
    }

    /// Every sign-in under way ends so.
    pub fn end_sign_ins(&mut self, ending: Ending, message: Option<&str>) {
        let attempts: Vec<String> = self.attempts.iter().filter(|(_, attempt)| attempt.status["status"] == "pending").map(|(id, _)| id.clone()).collect();
        for id in attempts {
            self.end(&id, ending, message);
        }
    }

    /// Forgets every sign-in under way, as an engine that started over does (its status: `404`).
    pub fn forget_sign_ins(&mut self) {
        self.attempts.clear();
    }

    /// From now on, a sign-in's status carries this device code, as the bridge says it once its
    /// page is ready.
    pub fn set_user_code(&mut self, code: &str) {
        for attempt in self.attempts.values_mut() {
            attempt.status["userCode"] = json!(code);
        }
    }

    fn end(&mut self, id: &str, ending: Ending, message: Option<&str>) {
        let Some(attempt) = self.attempts.get_mut(id) else { return };
        let (agents, account) = (attempt.agents, attempt.account.clone());
        attempt.status = match ending {
            Ending::Complete => json!({ "status": "complete" }),
            Ending::Failed => json!({ "status": "failed", "message": message, "failure": failure(message) }),
            Ending::Expired => json!({ "status": "expired", "failure": "The sign-in timed out. Try again." }),
        };
        if ending == Ending::Complete {
            self.connect(agents, &account, "oauth", "Signed in");
        }
    }

    fn list(&mut self, agents: bool) -> &mut Vec<Value> {
        if agents { &mut self.agents } else { &mut self.engine }
    }

    fn connect(&mut self, agents: bool, account: &str, method: &str, label: &str) {
        if let Some(found) = self.list(agents).iter_mut().find(|each| each["id"] == account) {
            found["connections"] = json!([{ "type": "credential", "id": "cred_new", "method": method, "label": label }]);
            if let Some(metadata) = found["metadata"].as_object_mut() {
                metadata.remove("signInEnded");
            }
        }
        self.ended.retain(|ended| ended != account);
    }

    fn ways_of(&self, agents: bool, account: &str) -> Option<Value> {
        let key = if agents { format!("agents:{account}") } else { account.to_owned() };
        self.ways.get(&key).cloned()
    }

    /// Its answer to `method route` (`query`: a query value by name), if it's one of these routes.
    pub fn answer(&mut self, method: &str, route: &str, query: &dyn Fn(&str) -> Option<String>, body: &Value, name: &str, here: bool) -> Option<(u16, Value)> {
        let segments: Vec<&str> = route.split('/').collect();
        if let Some(answer) = self.vault_answer(method, &segments, body) {
            return Some(answer);
        }
        if let Some(answer) = self.pages_answer(method, &segments, body, name, here) {
            return Some(answer);
        }
        let agents_side = route.starts_with("kuvo/agents/api/");
        let api: Vec<&str> = if agents_side { segments[3..].to_vec() } else if route.starts_with("api/") { segments[1..].to_vec() } else { Vec::new() };
        if agents_side && self.agents_down && api.first() == Some(&"integration") {
            return Some((503, json!({ "message": "The coding agents aren’t running." })));
        }
        if agents_side && api.as_slice() == ["kuvo", "accounts"] && method == "POST" {
            let provider = body["provider"].as_str().unwrap_or_default();
            let Some(mut account) = self.agents.iter().find(|each| each["id"] == provider).cloned() else {
                return Some((404, json!({ "message": "No such agent." })));
            };
            self.next += 1;
            let id = format!("{}@added{}", provider.split('@').next().unwrap_or(provider), self.next);
            account["id"] = json!(id);
            account["metadata"]["accountID"] = json!(format!("added{}", self.next));
            account["connections"] = json!([]);
            if let Some(ways) = self.ways_of(true, provider) { self.ways.insert(format!("agents:{id}"), ways); }
            self.agents.push(account);
            return Some((200, json!({ "data": { "providerID": id } })));
        }
        if agents_side && api.len() == 3 && api[0..2] == ["kuvo", "accounts"] && method == "DELETE" {
            let account = path_part(api[2]);
            self.agents.retain(|each| each["id"] != account.as_str() || (query("onlyUnsigned").as_deref() == Some("1") && each["connections"].as_array().is_some_and(|connections| !connections.is_empty())));
            return Some((200, json!({})));
        }
        if !api.is_empty()
            && let Some(answer) = self.integration_answer(method, agents_side, &api, body)
        {
            return Some(answer);
        }
        let answer = match (method, segments.as_slice()) {
            ("GET", ["kuvo", "sign-in", "accounts"]) => {
                let agents = query("agents").is_some();
                if agents && self.agents_down {
                    return Some((503, json!({ "message": "The coding agents aren’t running." })));
                }
                let list = if agents { &self.agents } else { &self.engine };
                let accounts: Vec<Value> = list
                    .iter()
                    .filter_map(|each| {
                        let id = each["id"].as_str()?;
                        let mut ways = self.ways_of(agents, id).unwrap_or_else(|| ways(json!([]), json!([]), "Connect"));
                        ways["id"] = json!(id);
                        // What it is, whether Accounts lists it, and its mark, as the box decides.
                        let (kind, listed) = kind_of(each, agents);
                        ways["kind"] = json!(kind);
                        ways["listed"] = json!(listed);
                        ways["plan"] = json!(is_plan(&ways));
                        if let Some(service) = service_of(each, agents) {
                            ways["service"] = json!(service);
                        }
                        // How it stands and what can be done with it: the saved sign-in in use
                        // of a plan whose sign-in ended is the one that did.
                        let ended = (!agents && self.ended.iter().any(|ended| ended == id)).then(|| each["connections"][0]["id"].as_str()).flatten();
                        let mut said = said_of(each, &ways, agents, ended, COMPUTER);
                        if let Some(set) = self.said.get(id).and_then(Value::as_object) {
                            for (key, value) in set {
                                said[key] = value.clone();
                            }
                        }
                        for key in ["state", "line", "actions", "connections"] {
                            ways[key] = said[key].clone();
                        }
                        if said["added"] == true {
                            ways["added"] = json!(true);
                        }
                        Some(ways)
                    })
                    .collect();
                (200, json!({ "accounts": accounts }))
            }
            ("GET", ["kuvo", "sign-in", "methods"]) => {
                let account = query("integration").unwrap_or_default();
                match self.ways_of(query("agents").is_some(), &account) {
                    Some(ways) => (200, ways),
                    None => (404, json!({ "message": "There’s no such account here." })),
                }
            }
            ("POST", ["kuvo", "sign-in", "start"]) => {
                let (agents, account) = (query("agents").is_some(), query("integration").unwrap_or_default());
                if agents && self.agents_down {
                    return Some((503, json!({ "message": "The coding agents aren’t running." })));
                }
                if account.is_empty() || body["methodID"].as_str().is_none_or(str::is_empty) {
                    return Some((400, json!({ "message": "Say which account (integration) and which way in (methodID)." })));
                }
                if !self.list(agents).iter().any(|each| each["id"] == account.as_str()) {
                    return Some((404, json!({ "message": "There’s no such account here." })));
                }
                (200, self.start(agents, &account, body))
            }
            ("GET", ["kuvo", "sign-in", "status"]) => {
                let attempt = query("attempt").unwrap_or_default();
                let Some(found) = self.attempts.get_mut(&attempt) else { return Some((404, json!({ "message": "That sign-in isn’t known here." }))) };
                found.asked += 1;
                if self.finishes && found.asked >= 3 && found.status["status"] == "pending" {
                    self.end(&attempt, Ending::Complete, None);
                }
                (200, self.attempts[&attempt].status.clone())
            }
            ("GET", ["kuvo", "accounts", "ended"]) => (200, json!({ "providers": self.ended })),
            ("POST", ["kuvo", "accounts", provider, "forget"]) => {
                let provider = path_part(provider);
                match self.engine.iter_mut().find(|each| each["id"] == provider.as_str()) {
                    Some(found) => {
                        found["connections"] = json!([]);
                        self.ended.retain(|ended| *ended != provider);
                        (200, json!({}))
                    }
                    None => (404, json!({ "message": "That isn’t a plan here." })),
                }
            }
            ("GET", ["kuvo", "config"]) => (200, self.config.clone()),
            ("PATCH", ["kuvo", "config"]) => {
                for change in body["changes"].as_array().cloned().unwrap_or_default() {
                    let path: Vec<String> = change["path"].as_array().map(|parts| parts.iter().filter_map(|part| part.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                    let now = path.iter().try_fold(&self.config, |value, key| value.get(key)).cloned().unwrap_or(Value::Null);
                    if change.get("was").is_some() && change["was"] != now {
                        return Some((409, json!({ "message": "This setting changed on its computer since. Read it again." })));
                    }
                    set_at(&mut self.config, &path, change["value"].clone());
                }
                (200, self.config.clone())
            }
            ("GET", ["kuvo", "approval"]) => (200, super::work_bars::with_options(&self.approval)),
            // (Full Access confirmed on this computer, from a composer's mode menu; your other
            // computers' starts, from Settings.)
            ("PATCH", ["kuvo", "approval"]) => {
                let (full, others) = (body.get("fullAccess").and_then(Value::as_bool), body.get("otherComputers").and_then(Value::as_bool));
                if full.is_none() && others.is_none() {
                    return Some((400, json!({ "message": "Say otherComputers as true or false." })));
                }
                if let Some(full) = full {
                    self.approval["fullAccess"] = json!(full);
                }
                if let Some(allowed) = others {
                    self.approval["otherComputers"] = json!(allowed);
                }
                (200, super::work_bars::with_options(&self.approval))
            }
            ("GET", ["kuvo", "sandbox"]) => (200, self.sandbox.clone()),
            ("POST", ["kuvo", "sandbox", "check"]) => (200, json!({})),
            ("GET", ["kuvo", "sandbox", "allowed"]) => (200, json!({ "allowed": self.allowed })),
            ("DELETE", ["kuvo", "sandbox", "allowed", id]) => {
                let before = self.allowed.len();
                self.allowed.retain(|rule| rule["id"] != *id);
                if self.allowed.len() == before { (404, json!({ "message": "That folder isn’t allowed here." })) } else { (200, json!({})) }
            }
            ("GET", ["kuvo", "browser", "settings"]) => (200, self.browser.clone()),
            ("PATCH", ["kuvo", "browser", "settings"]) => {
                if let Some(engine) = body.get("searchEngine").and_then(Value::as_str) {
                    let known = self.browser["searchEngines"].as_array().is_some_and(|engines| engines.iter().any(|each| each["id"] == engine));
                    if !known {
                        return Some((400, json!({ "message": "That isn’t a search engine the browser knows." })));
                    }
                }
                for key in ["searchEngine", "tidiesAgentTabs", "offersToSave", "never"] {
                    if let Some(value) = body.get(key) {
                        self.browser[key] = value.clone();
                    }
                }
                (200, self.browser.clone())
            }
            ("GET", ["kuvo", "agents", "api", "kuvo", "usage", "settings"]) => (200, self.usage_answer(&[])),
            ("PUT", ["kuvo", "agents", "api", "kuvo", "usage", "settings"]) => {
                let based = body.pointer("/based/notifications").and_then(Value::as_str).map(str::to_owned);
                if based.as_deref() != Some(self.usage_version().as_str()) {
                    return Some((200, self.usage_answer(&["notifications"])));
                }
                if let Some(part) = body.get("notifications") {
                    self.usage = part.clone();
                    self.usage_version += 1;
                }
                (200, self.usage_answer(&[]))
            }
            _ => return None,
        };
        Some(answer)
    }

    /// A section added to its settings (`section`: its line in `kuvo/settings`; `page`: its page,
    /// each row's revision worked out as it's read), for kinds its own pages don't have yet.
    pub fn add_section(&mut self, section: Value, page: Value) {
        self.more_sections.push((section, page));
    }

    /// The folders also allowed, in place of the sample's.
    pub fn set_allowed(&mut self, allowed: Vec<Value>) {
        self.allowed = allowed;
    }

    /// A browser setting changes, as from another client meanwhile.
    pub fn set_browser(&mut self, key: &str, value: Value) {
        self.browser[key] = value;
    }

    pub fn set_sandbox(&mut self, sandbox: Value) {
        self.sandbox = sandbox;
    }

    pub fn set_account_said(&mut self, account: &str, said: Value) {
        self.said.insert(account.to_owned(), said);
    }

    pub fn set_config(&mut self, path: &[&str], value: Value) {
        let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
        set_at(&mut self.config, &path, value);
    }

    fn usage_version(&self) -> String {
        format!("{}/mac", self.usage_version)
    }

    fn usage_answer(&self, stale: &[&str]) -> Value {
        json!({ "data": {
            "settings": { "notifications": self.usage },
            "versions": { "notifications": self.usage_version() },
            "imported": true,
            "stale": stale,
            "notificationAvailability": [{ "provider": "devin", "available": false, "message": "Devin doesn’t share its limits, so it can’t warn you before they run out." }],
        } })
    }

    /// Starts a sign-in in the browser, as the box does: the attempt its engine (or bridge)
    /// answers, with the code its page asks for apart (`userCode`). The engine leaves a device
    /// sign-in's code in its page's address; the box says it.
    fn start(&mut self, agents: bool, account: &str, body: &Value) -> Value {
        let id = format!("att_{}", self.next);
        self.next += 1;
        let method_id = body["methodID"].as_str().unwrap_or_default();
        let device = method_id == "device";
        let accepts_code = agents && account.starts_with("claude-code");
        let mut attempt = json!({
            "attemptID": id, "url": format!("https://sign-in.stand-in.invalid/{account}/{method_id}{}", if device { "?user_code=WDJB-MJHT" } else { "" }),
            "instructions": if device { "Enter the code on the page." } else { "" },
            "mode": "auto", "acceptsCode": accepts_code,
            "time": { "created": 0, "expires": 900_000 },
        });
        let mut status = if account == "poe" {
            json!({ "status": "failed", "message": "access_denied", "failure": failure(Some("access_denied")) })
        } else {
            json!({ "status": "pending" })
        };
        if device {
            attempt["userCode"] = json!("WDJB-MJHT");
            status["userCode"] = json!("WDJB-MJHT");
        }
        self.attempts.insert(id, Attempt { agents, account: account.to_owned(), status, asked: 0 });
        attempt
    }

    fn integration_answer(&mut self, method: &str, agents: bool, api: &[&str], body: &Value) -> Option<(u16, Value)> {
        let account = api.get(1).map(|part| path_part(part)).unwrap_or_default();
        let known = |this: &Self| (if agents { &this.agents } else { &this.engine }).iter().any(|each| each["id"] == account.as_str());
        let answer = match (method, api) {
            ("GET", ["integration"]) => (200, json!({ "data": if agents { &self.agents } else { &self.engine } })),
            ("POST", ["integration", _, "connect", "oauth", attempt, "complete"]) => {
                let attempt = path_part(attempt);
                if !self.attempts.contains_key(&attempt) {
                    return Some((404, json!({ "message": "That sign-in isn’t known here." })));
                }
                if body["code"].as_str().is_none_or(|code| code.trim().is_empty() || code.contains("wrong")) {
                    return Some((400, json!({ "message": "That code wasn’t taken. Paste the whole code the page gave." })));
                }
                self.end(&attempt, Ending::Complete, None);
                (200, json!({ "data": true }))
            }
            ("DELETE", ["integration", _, "connect", "oauth", attempt]) => {
                self.attempts.remove(&path_part(attempt));
                (200, json!({ "data": true }))
            }
            ("POST", ["integration", _, "connect", "key"]) if known(self) => {
                let key = body["key"].as_str().unwrap_or_default();
                if !key.starts_with("sk-") {
                    return Some((400, json!({ "message": "That key wasn’t accepted (401 Unauthorized)." })));
                }
                self.connect(agents, &account, "key", "API key");
                (200, json!({ "data": true }))
            }
            ("POST", ["integration", _, "connect", "external"]) if known(self) => {
                self.connect(agents, &account, "external", "AWS credentials");
                (200, json!({ "data": true }))
            }
            ("POST", ["integration", _, "install"]) if agents && known(self) => {
                if let Some(found) = self.agents.iter_mut().find(|each| each["id"] == account.as_str()) {
                    found["metadata"]["installed"] = json!(true);
                    found["metadata"]["version"] = json!("1.2.0");
                }
                (200, json!({ "data": { "installed": true, "version": "1.2.0" } }))
            }
            ("POST", ["integration", _, "disconnect"]) if agents && known(self) => {
                if let Some(found) = self.agents.iter_mut().find(|each| each["id"] == account.as_str()) {
                    found["connections"] = json!([]);
                }
                (200, json!({ "data": true }))
            }
            ("GET" | "POST" | "DELETE", ["integration", ..]) => (404, json!({ "message": "There’s no such account here." })),
            _ => return None,
        };
        Some(answer)
    }
}

/// What an account is, and whether Accounts lists it, as the box decides (PROTOCOL.md, "Signing
/// in"): a web search service and an MCP server's sign-in aren't listed; the engine's own way to a
/// plan a coding agent brings, only while it's signed in.
fn kind_of(account: &Value, agents: bool) -> (&'static str, bool) {
    match account["id"].as_str().unwrap_or_default() {
        _ if agents => ("provider", true),
        "exa" | "firecrawl" | "parallel" | "tavily" | "tinyfish" => ("search", false),
        _ if account["metadata"]["source"] == "mcp" => ("mcp", false),
        "claude-agent" => ("agentDuplicate", account["connections"].as_array().is_some_and(|connections| !connections.is_empty())),
        _ => ("provider", true),
    }
}

/// An account's mark, by the plan limits' service ids: a coding agent's is its agent's; the
/// engine's, its maker's where it has one.
fn service_of(account: &Value, agents: bool) -> Option<&str> {
    if agents {
        return account["metadata"]["agent"].as_str();
    }
    Some(match account["id"].as_str()? {
        "anthropic" | "claude-agent" => "claude",
        "github-copilot" => "copilot",
        "openai" => "codex",
        "openrouter" => "openrouter",
        "xai" => "grok",
        _ => return None,
    })
}

/// The stand-in's computer, as its box names it in a sentence.
const COMPUTER: &str = super::NAME;

/// Whether an account's only ways in are keys, as the box offers them.
fn key_only(offered: &Value) -> bool {
    offered["methods"].as_array().is_some_and(|methods| !methods.is_empty() && methods.iter().all(|method| method["type"] == "key"))
}

/// Whether an account is reached with the computer's credentials (a key perhaps beside them) and
/// no sign-in page: one you connect, not sign in to.
fn connects(offered: &Value) -> bool {
    let methods = offered["methods"].as_array().map(Vec::as_slice).unwrap_or_default();
    methods.iter().any(|method| method["type"] == "external") && !methods.iter().any(|method| method["type"] == "oauth")
}

/// Whether an account is a plan you sign in to: one of its ways in is its sign-in page
/// (`sign-in.accounts.plan`).
pub fn is_plan(offered: &Value) -> bool {
    offered["methods"].as_array().is_some_and(|methods| methods.iter().any(|method| method["type"] == "oauth"))
}

/// How an account stands, its line, its sign-ins and what can be done with it, as the box says
/// them (box/src/link/areas/sign-in-state.ts; PROTOCOL.md, `sign-in.accounts.state`): the stand-in
/// answers what a box would for its made-up accounts, held to the same cases
/// (`Vectors/sign-in.json`, `states`). `offered`: its ways in; `ended`: its saved sign-in the box
/// saw end.
pub fn said_of(account: &Value, offered: &Value, agents: bool, ended: Option<&str>, computer: &str) -> Value {
    let text = |value: &Value| value.as_str().map(|text| text.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let id = account["id"].as_str().unwrap_or_default();
    let name = Some(text(&account["name"])).filter(|name| !name.is_empty()).unwrap_or_else(|| id.to_owned());
    let meta = &account["metadata"];
    let listed: Vec<&Value> = account["connections"].as_array().map(|all| all.iter().filter(|each| each.is_object()).collect()).unwrap_or_default();
    // Each sign-in: (its answer, who, whether it reads "Signed in as", what's wrong).
    let said: Vec<(Value, String, bool, String)> = listed
        .iter()
        .map(|each| {
            let label = text(&each["label"]);
            let kind = match each["method"].as_str() {
                Some("key") => "API key".to_owned(),
                Some("external") => format!("Credentials on {computer}"),
                _ => String::new(),
            };
            let who = if label != name && label.to_lowercase() != kind.to_lowercase() { label } else { String::new() };
            let line = if each["type"] == "env" {
                format!("From {}", Some(text(&each["name"])).filter(|name| !name.is_empty()).unwrap_or_else(|| "its environment".into()))
            } else if !kind.is_empty() {
                if who.is_empty() { kind.clone() } else { format!("{kind} · {who}") }
            } else if who.is_empty() {
                "Signed in".into()
            } else {
                who.clone()
            };
            let trouble = if each["status"]["status"] == "failed" { text(&each["status"]["message"]) } else { String::new() };
            let saved = (each["type"] == "credential").then(|| each["id"].as_str()).flatten().filter(|id| !id.is_empty());
            let is_ended = (!agents && saved.is_some() && saved == ended) || each["status"]["status"] == "needs_auth";
            let state = if is_ended { "ended" } else if !trouble.is_empty() { "problem" } else { "signedIn" };
            let mut answer = json!({ "state": state, "line": line });
            if let Some(saved) = saved {
                answer["id"] = json!(saved);
            }
            (answer, who.clone(), each["type"] != "env" && kind.is_empty() && !who.is_empty(), trouble)
        })
        .collect();
    let in_use = said.first();
    let saved_in_use = listed.iter().position(|each| each["type"] == "credential").and_then(|at| said.get(at));
    let signs_in = offered["methods"].as_array().is_some_and(|methods| !methods.is_empty());
    // Keys are entered, never signed into: a key-only account's buttons say so; one reached with
    // the computer's credentials is connected.
    let (first, again, another) = if key_only(offered) {
        ("Add Key", "Replace Key", "Add Another Key")
    } else if connects(offered) {
        ("Connect", "Reconnect", "Add Another Account")
    } else {
        ("Sign In", "Sign In Again", "Add Another Account")
    };
    let added = agents && (id.contains('@') || meta["accountID"].as_str().is_some_and(|account| !account.is_empty() && account != "default"));
    let installs = agents && meta["installed"] == false && meta["install"].as_str().is_some_and(|install| !install.is_empty());
    let state = if installs {
        "notInstalled"
    } else if if agents { meta["signInEnded"] == true } else { in_use.is_some_and(|first| first.0["state"] == "ended") || saved_in_use.is_some_and(|saved| saved.0["state"] == "ended") } {
        "ended"
    } else if let Some(first) = in_use {
        if first.0["state"] == "problem" { "problem" } else { "signedIn" }
    } else if agents && meta["unchecked"] == true {
        "unchecked"
    } else {
        "signedOut"
    };
    let environment: Vec<String> = offered["environment"].as_array().map(|all| all.iter().filter_map(|each| each.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let line = match (state, in_use) {
        ("notInstalled", _) => format!("Not installed on {computer}"),
        ("ended", _) => "Sign-in ended".into(),
        ("problem", Some(first)) => first.3.clone(),
        ("unchecked", _) => "Couldn’t check just now".into(),
        ("signedIn", Some(first)) if agents && meta["unchecked"] == true && first.1.is_empty() => "Couldn’t check just now".into(),
        ("signedIn", Some(first)) => {
            let line = if first.2 { format!("Signed in {} {}", if listed[0]["what"] == true { "with" } else { "as" }, first.1) } else { first.0["line"].as_str().unwrap_or_default().to_owned() };
            if said.len() > 1 { format!("{line} · {} accounts", said.len()) } else { line }
        }
        _ if agents => "Not signed in".into(),
        _ if !signs_in && !environment.is_empty() => format!("Set {} in your shell profile", environment.join(" or ")),
        _ if !signs_in => "Not signed in".into(),
        _ => Some(text(&offered["hint"])).filter(|hint| !hint.is_empty()).unwrap_or_else(|| "Not signed in".into()),
    };
    let mut actions = Vec::new();
    let sign_in = |id: &str, label: &str| {
        let mut action = json!({ "id": id, "label": label });
        if agents && meta["remoteSignIn"] != true {
            action["here"] = json!(true);
        }
        action
    };
    if agents {
        if installs {
            actions.push(json!({ "id": "install", "label": "Install" }));
        } else if signs_in {
            actions.push(if !said.is_empty() || state == "ended" { sign_in("signInAgain", again) } else { sign_in("signIn", first) });
        }
        if !added && !installs && signs_in && meta["addedAccounts"] == true {
            actions.push(sign_in("addAnother", another));
        }
        let latest = text(&meta["latestVersion"]);
        if !added && !installs && meta["outdated"] == true && meta["updates"] == true && !latest.is_empty() && meta["programUpdate"]["state"] != "updating" {
            actions.push(json!({ "id": "update", "label": format!("Update to {latest}") }));
        }
        if meta["signsOut"] == true && !said.is_empty() {
            actions.push(json!({ "id": "signOut", "label": "Sign Out" }));
        }
        if added {
            actions.push(json!({ "id": "rename", "label": "Rename" }));
            actions.push(json!({ "id": "remove", "label": "Remove Account", "confirm": { "title": format!("Remove {name}?"), "message": format!("It’s signed out on {computer}, and its models leave the model picker. A turn running on it stops; its sessions stay."), "label": "Remove Account" } }));
        }
    } else if said.is_empty() {
        if signs_in {
            actions.push(json!({ "id": "signIn", "label": first }));
        }
    } else {
        if signs_in {
            if state == "ended" || state == "problem" || said.iter().any(|each| each.0["state"] == "ended") {
                actions.push(json!({ "id": "signInAgain", "label": again }));
            }
            actions.push(json!({ "id": "addAnother", "label": another }));
        }
        if meta["source"] != "mcp" && listed.iter().any(|each| each["type"] == "credential") {
            let stays: Vec<String> = listed.iter().filter(|each| each["type"] == "env").map(|each| text(&each["name"])).filter(|name| !name.is_empty()).collect();
            let message = if stays.is_empty() {
                format!("Its sign-ins and keys go from {computer}, and its models leave the model picker there. Sessions on its models stop until you sign in again.")
            } else {
                format!("Its saved sign-ins and keys go from {computer}. It stays signed in from {} in its shell’s environment, so its models stay.", stays.join(" and "))
            };
            actions.push(json!({ "id": "disconnect", "label": "Disconnect", "confirm": { "title": format!("Disconnect {name}?"), "message": message, "label": "Disconnect" } }));
        }
    }
    for action in &mut actions {
        if matches!(action["id"].as_str(), Some("signIn" | "signInAgain" | "addAnother")) {
            action["sheetTitle"] = json!(if key_only(offered) || connects(offered) { format!("{} · {name}", action["label"].as_str().unwrap_or_default()) } else { format!("Sign In to {name}") });
        }
    }
    let mut answer = json!({ "state": state, "line": line, "actions": actions, "connections": said.into_iter().map(|each| each.0).collect::<Vec<_>>() });
    if added {
        answer["added"] = json!(true);
    }
    answer
}

/// What the box says of a failed sign-in, as `Vectors/sign-in.json` words the cases used here.
fn failure(message: Option<&str>) -> String {
    match message {
        Some(message) if message.contains("access_denied") => "The sign-in was turned down on its page. Try again to sign in.".into(),
        Some(message) if !message.trim().is_empty() => message.to_owned(),
        _ => "Signing in didn’t work.".into(),
    }
}

/// Sets `value` at `path`, making the levels that aren't there; `null` removes it.
fn set_at(document: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let mut at = document;
    for key in parents {
        if !at.get(key).is_some_and(Value::is_object) {
            at[key.as_str()] = Value::Object(Map::new());
        }
        at = &mut at[key.as_str()];
    }
    if value.is_null() {
        if let Some(object) = at.as_object_mut() {
            object.remove(last);
        }
    } else {
        at[last.as_str()] = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stand-in says of an account what a box would: every case the box is held to.
    #[test]
    fn the_stand_in_says_an_accounts_state_as_the_box_does() {
        let vectors: Value = serde_json::from_str(include_str!("../../../../protocol/vectors/sign-in.json")).unwrap();
        let cases = vectors["states"].as_array().unwrap();
        assert!(cases.len() >= 41, "every case the box has: one added there is one to hold this to");
        for case in cases {
            let said = said_of(&case["account"], &case["offered"], case["agents"] == true, case["ended"].as_str(), case["computer"].as_str().unwrap());
            for key in ["state", "line", "actions", "connections", "added"] {
                assert_eq!(said[key], case[key], "{}: {key}", case["about"]);
            }
            assert_eq!(json!(is_plan(&case["offered"])), case["plan"], "{}: plan", case["about"]);
        }
    }
}
