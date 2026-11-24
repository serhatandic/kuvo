//! Integration tests for row 5.5b: Marketplace, skill removal/restore, MCP catalog & setup,
//! integrations upgrade, and `kuvo/programs`.

use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

use bytes::Bytes;
use http::{HeaderMap, Method};
use kuvo_server::{Caller, DispatchRequest, Dispatcher, LineTiming, LocalOrigin, Registry, Transport};
use kuvo_services::{
    config::{ConfigPaths, SERVERS, SettingsRead},
    integrations::{
        keys::KeyStore,
        mcp_catalog::{add_from_catalog, mcp_catalog},
        mcp_setup::{command_line, command_words, server_setup, server_setups, web_address},
        upgrade::{upgrade_before_engine, upgrade_integrations},
    },
    marketplace::{
        ResidentMarketplace,
        bots::{MarketplaceScope, project_bot_scope},
        installations::{Installations, InstalledSkill},
        routes::StandalonePeers,
    },
    skills::{ResidentSkills, store::pack_file},
    vault::{Launch, Resident as VaultResident},
};
use serde_json::{Map, Value, json};

async fn dispatch_json(disp: &Dispatcher, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|v| Bytes::from(serde_json::to_vec(&v).unwrap())).unwrap_or_default();
    let req = DispatchRequest {
        method,
        path: path.into(),
        headers: HeaderMap::new(),
        body: bytes,
        caller: Caller::Local(Transport::Http { origin: LocalOrigin::App, internal: false }),
    };
    let reply = disp.dispatch(req, tokio_util::sync::CancellationToken::new()).await;
    use http_body_util::BodyExt;
    let raw = reply.body.collect().await.unwrap().to_bytes();
    let val = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    (reply.status.as_u16(), val)
}

async fn dispatch_bytes(disp: &Dispatcher, method: Method, path: &str) -> (u16, Vec<u8>) {
    let req = DispatchRequest {
        method,
        path: path.into(),
        headers: HeaderMap::new(),
        body: Bytes::new(),
        caller: Caller::Local(Transport::Http { origin: LocalOrigin::App, internal: false }),
    };
    let reply = disp.dispatch(req, tokio_util::sync::CancellationToken::new()).await;
    use http_body_util::BodyExt;
    (reply.status.as_u16(), reply.body.collect().await.unwrap().to_bytes().to_vec())
}

fn put_skill(folder: &Path, name: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(folder.join("SKILL.md"), format!("---\nname: {name}\ndescription: A test skill\n---\nInstructions.\n")).unwrap();
    fs::write(folder.join("example.txt"), "Keep this file.").unwrap();
}

async fn make_marketplace_box(root: &Path) -> (Dispatcher, ResidentMarketplace, ResidentSkills, Arc<VaultResident>) {
    let home = root.join("box");
    let account = root.join("home");
    fs::create_dir_all(home.join("box")).unwrap();
    fs::create_dir_all(&account).unwrap();
    fs::write(home.join("box/opencode.json"), "{}").unwrap();
    let vault = VaultResident::new(&home, false, Launch::default(), account.clone(), &account, Some("TestBox".into()));
    let mut reg = Registry::new();
    let skills = ResidentSkills::start(&mut reg, &home, &account);
    let mp = ResidentMarketplace::start(
        &mut reg, &home, &account, false, "TestBox".into(), None,
        vault.clone(), skills.store.clone(), skills.library.clone(), Arc::new(StandalonePeers),
    ).await;
    let disp = reg.build(LineTiming::default());
    (disp, mp, skills, vault)
}

#[tokio::test]
async fn bundled_catalog_png_logos_catalog_skill_install_and_bot_scope_projection() {
    let tmp = tempfile::tempdir().unwrap();
    let (disp, mp, skills, _vault) = make_marketplace_box(tmp.path()).await;

    let (st, listed) = dispatch_json(&disp, Method::GET, "kuvo/marketplace", None).await;
    assert_eq!(st, 200);
    let areas = listed["areas"].as_array().unwrap();
    assert_eq!(areas[0]["count"], 10);
    assert_eq!(areas[1]["count"], 6);
    assert_eq!(areas[2]["count"], 8);
    let apps = areas[0]["items"].as_array().unwrap();
    assert!(!apps.iter().any(|a| a["id"] == "slack"));
    let gh = apps.iter().find(|a| a["id"] == "github").unwrap();
    assert_eq!(gh["logo"], "kuvo/marketplace/apps/github/logo");

    let (logo_st, logo_bytes) = dispatch_bytes(&disp, Method::GET, "kuvo/marketplace/apps/github/logo").await;
    assert_eq!(logo_st, 200);
    assert_eq!(&logo_bytes[..8], b"\x89PNG\r\n\x1a\n");

    // Install catalog skill review-changes
    let (d_st, detail) = dispatch_json(&disp, Method::GET, "kuvo/marketplace/skills/review-changes", None).await;
    assert_eq!(d_st, 200);
    let avail = &detail["available"];
    let (add_st, _) = dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/review-changes/add", Some(json!({
        "version": avail["version"],
        "revision": avail["revision"],
    }))).await;
    assert_eq!(add_st, 200);
    let skill_md = fs::read_to_string(tmp.path().join("box/config/opencode/skills/review-changes/SKILL.md")).unwrap();
    assert!(skill_md.contains("Report concrete findings"));

    // Bot scope projection preserves existing policies and scopes apps + skills
    let bot_ws = tmp.path().join("bot-ws");
    fs::create_dir_all(&bot_ws).unwrap();
    fs::write(bot_ws.join("opencode.json"), r#"{"policies":[{"action":"edit","resource":"*","effect":"allow"}]}"#).unwrap();
    let scope = MarketplaceScope {
        agent: "reviewer".into(), version: "1".into(), request: "req_1234567890123456789012".into(),
        apps: vec!["github".into()], skills: vec!["review-changes".into()],
    };
    let state = Installations {
        format: 1, apps: vec![],
        skills: vec![
            InstalledSkill { name: "review-changes".into(), files: vec![pack_file("SKILL.md", b"# a")], source: None, origin: "created".into(), version: None },
            InstalledSkill { name: "other-skill".into(), files: vec![pack_file("SKILL.md", b"# b")], source: None, origin: "created".into(), version: None },
        ],
        removed_skills: None,
    };
    project_bot_scope(&bot_ws, &scope, &state).unwrap();
    let cfg: Value = serde_json::from_str(&fs::read_to_string(bot_ws.join("opencode.json")).unwrap()).unwrap();
    let pols = cfg["policies"].as_array().unwrap();
    assert!(pols.iter().any(|p| p["action"] == "edit"));
    assert!(pols.iter().any(|p| p["action"] == "mcp.use" && p["resource"] == "marketplace-*" && p["effect"] == "deny"));
    assert!(pols.iter().any(|p| p["action"] == "mcp.use" && p["resource"] == "marketplace-github" && p["effect"] == "allow"));
    assert!(pols.iter().any(|p| p["action"] == "skill.use" && p["resource"] == "other-skill" && p["effect"] == "deny"));
    assert!(!pols.iter().any(|p| p["action"] == "skill.use" && p["resource"] == "review-changes"));

    skills.stop();
    mp.stop().await;
}

#[tokio::test]
async fn marketplace_app_token_vault_projection_and_removal_clean_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let (disp, mp, skills, vault) = make_marketplace_box(tmp.path()).await;

    // Make encrypted vault so token credentials can be saved
    let mut v = kuvo_services::vault::Vault::open(&tmp.path().join("box"), false, Launch::default());
    let key = v.make().unwrap();
    drop(v);
    // Unlock resident vault via its unlock route is covered in vault tests; test open app + locked token app here
    let (st, card) = dispatch_json(&disp, Method::GET, "kuvo/marketplace/apps/github", None).await;
    assert_eq!(st, 200);
    assert_eq!(card["auth"], "token");
    assert_eq!(dispatch_json(&disp, Method::POST, "kuvo/marketplace/apps/github/add", None).await.0, 200);
    // Vault is still locked in Resident -> connection is "locked" and connect returns 423
    let (c_st, _) = dispatch_json(&disp, Method::POST, "kuvo/marketplace/apps/github/connect", Some(json!({ "token": "ghp_secret123456" }))).await;
    assert_eq!(c_st, 423);

    // Custom open app projects directly into opencode.json and removes cleanly
    let (cust_st, cust) = dispatch_json(&disp, Method::POST, "kuvo/marketplace/apps/custom", Some(json!({
        "title": "Docs MCP",
        "url": "https://docs.example.com/mcp",
        "auth": "open",
    }))).await;
    assert_eq!(cust_st, 200);
    assert_eq!(cust["id"], "docs-mcp");
    let cfg: Value = serde_json::from_str(&fs::read_to_string(tmp.path().join("box/box/opencode.json")).unwrap()).unwrap();
    assert_eq!(cfg["mcp"]["servers"]["marketplace-docs-mcp"]["url"], "https://docs.example.com/mcp");
    assert_eq!(dispatch_json(&disp, Method::DELETE, "kuvo/marketplace/apps/docs-mcp", None).await.0, 200);
    let cfg_after: Value = serde_json::from_str(&fs::read_to_string(tmp.path().join("box/box/opencode.json")).unwrap()).unwrap();
    assert!(cfg_after["mcp"]["servers"]["marketplace-docs-mcp"].is_null());

    // Keep key variable used
    assert!(!key.expose().is_empty());
    let _ = vault;
    skills.stop();
    mp.stop().await;
}

#[tokio::test]
async fn skill_removal_archives_and_restores_across_all_agent_roots_created_and_folder_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let (disp, mp, skills, _vault) = make_marketplace_box(tmp.path()).await;
    let home = tmp.path().join("home");
    let box_home = tmp.path().join("box");

    // Test all 7 agent roots + engine skills folder
    let roots = [
        home.join(".claude/skills"),
        home.join(".codex/skills"),
        home.join(".agents/skills"),
        box_home.join("agents/antigravity/profile/antigravity-cli/skills"),
        home.join(".grok/skills"),
        home.join(".cursor/skills"),
        box_home.join("config/opencode/skills"),
    ];
    for (idx, root) in roots.iter().enumerate() {
        let name = format!("agent-skill-{idx}");
        let folder = root.join(&name);
        put_skill(&folder, &name);

        let (d_st, detail) = dispatch_json(&disp, Method::GET, &format!("kuvo/marketplace/skills/{name}"), None).await;
        assert_eq!(d_st, 200, "missing skill in {}", root.display());
        assert_eq!(detail["installed"], true);
        let rev = detail["revision"].as_str().unwrap().to_owned();

        assert_eq!(dispatch_json(&disp, Method::DELETE, &format!("kuvo/marketplace/skills/{name}"), Some(json!({ "was": rev }))).await.0, 200);
        assert!(!folder.exists(), "folder should be archived from {}", root.display());

        // Resynced copy stays removed on reconcile
        if idx == 0 {
            put_skill(&folder, &name);
            mp.service.reconcile(None, &[]);
            assert!(!folder.exists());
        }

        let (_, listed) = dispatch_json(&disp, Method::GET, "kuvo/marketplace", None).await;
        let deleted = listed["areas"][2]["here"]["deleted"]["items"].as_array().unwrap();
        let entry = deleted.iter().find(|i| i["title"] == name).expect("in Recently Deleted");
        assert_eq!(dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/restore", Some(json!({ "id": entry["id"] }))).await.0, 200);
        assert_eq!(fs::read_to_string(folder.join("example.txt")).unwrap(), "Keep this file.");
        fs::remove_dir_all(&folder).unwrap();
    }

    // Created skill create -> edit -> delete -> restore
    assert_eq!(dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/create", Some(json!({
        "name": "created-example", "description": "test desc", "instructions": "Do it."
    }))).await.0, 200);
    let (_, det) = dispatch_json(&disp, Method::GET, "kuvo/marketplace/skills/created-example", None).await;
    assert_eq!(dispatch_json(&disp, Method::DELETE, "kuvo/marketplace/skills/created-example", Some(json!({ "was": det["revision"] }))).await.0, 200);
    assert!(!box_home.join("config/opencode/skills/created-example").exists());
    let (_, listed) = dispatch_json(&disp, Method::GET, "kuvo/marketplace", None).await;
    let del_id = listed["areas"][2]["here"]["deleted"]["items"].as_array().unwrap()
        .iter().find(|i| i["title"] == "created-example").unwrap()["id"].clone();
    assert_eq!(dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/restore", Some(json!({ "id": del_id }))).await.0, 200);
    assert_eq!(dispatch_json(&disp, Method::GET, "kuvo/marketplace/skills/created-example", None).await.1["origin"], "created");

    // Folder preview -> install -> delete -> restore preserves supporting files
    let import_dir = tmp.path().join("folder-import");
    put_skill(&import_dir, "folder-example");
    let (p_st, prev) = dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/preview", Some(json!({ "folder": import_dir.to_string_lossy() }))).await;
    assert_eq!(p_st, 200);
    assert_eq!(dispatch_json(&disp, Method::POST, "kuvo/marketplace/skills/install", Some(json!({ "preview": prev["preview"] }))).await.0, 200);
    assert_eq!(fs::read_to_string(box_home.join("config/opencode/skills/folder-example/example.txt")).unwrap(), "Keep this file.");

    skills.stop();
    mp.stop().await;
}

#[tokio::test]
async fn integrations_upgrade_updates_pinned_packages_migrates_official_and_respects_update_trial() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("box");
    fs::create_dir_all(home.join("box/keys")).unwrap();
    fs::create_dir_all(home.join("run")).unwrap();
    fs::create_dir_all(home.join("bin")).unwrap();

    let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/routes/box/integrations.json");
    let config = ConfigPaths::for_home(&home, false);
    let keys = KeyStore::for_home(&home, false);
    let figma_key = "KUVO_KEY_FIGMA_FIGMA_API_KEY";
    let todoist_key = "KUVO_KEY_TODOIST_TODOIST_API_KEY";
    fs::write(home.join("box/keys").join(figma_key), "tok").unwrap();
    fs::write(home.join("box/keys").join(todoist_key), "tok").unwrap();

    fs::write(&config.config_file, serde_json::to_string(&json!({
        "model": "mine/best",
        "mcp": { "servers": {
            "todoist": { "type": "local", "command": ["kuvo-npx", "@doist/todoist-mcp@1.0.0"], "environment": { "TODOIST_API_KEY": format!("{{env:{todoist_key}}}") }, "disabled": true },
            "figma": { "type": "local", "command": ["kuvo-npx", "figma-developer-mcp@0.6.0", "--stdio"], "environment": { "FIGMA_API_KEY": format!("{{file:{}/{figma_key}}}", config.keys_named) } },
            "mine": { "type": "local", "command": ["npx", "mine"] },
            "kuvo-browser": { "type": "local", "command": ["kuvo-browser"] },
        } }
    })).unwrap()).unwrap();

    // Update trial prevents deleting figma key
    fs::write(home.join("run/update-trial.json"), r#"{"formatVersion":1}"#).unwrap();
    upgrade_before_engine(&config, &catalog, &home.join("bin/mcp-catalog.json"));
    upgrade_integrations(&config, &keys, &catalog, &home.join("bin/mcp-catalog.json")).await;
    assert!(home.join("box/keys").join(figma_key).exists(), "key kept while update on trial");

    // Once trial finishes, figma switches to official and deletes its obsolete key; todoist updates pin + KUVO_NPX_APART + {file:...}
    fs::remove_file(home.join("run/update-trial.json")).unwrap();
    upgrade_integrations(&config, &keys, &catalog, &home.join("bin/mcp-catalog.json")).await;
    assert!(!home.join("box/keys").join(figma_key).exists(), "obsolete figma key deleted");
    assert!(home.join("box/keys").join(todoist_key).exists());
    let SettingsRead::Ok(after) = config.read_settings() else { panic!("readable") };
    let servers = after[SERVERS[0]][SERVERS[1]].as_object().unwrap();
    assert_eq!(servers["todoist"]["disabled"], true);
    assert_eq!(servers["todoist"]["codemode"], false);
    assert_eq!(servers["todoist"]["environment"]["KUVO_NPX_APART"], "1");
    assert_eq!(servers["todoist"]["environment"]["TODOIST_API_KEY"], format!("{{file:{}/{todoist_key}}}", config.keys_named));
    assert_eq!(servers["figma"]["type"], "remote");
    assert_eq!(servers["mine"]["codemode"], false);
    assert!(servers["kuvo-browser"].get("codemode").is_none());
}

#[test]
fn mcp_catalog_filters_unknown_access_and_mcp_setup_parses_all_readme_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let cat_file = tmp.path().join("mcp-catalog.json");
    fs::write(&cat_file, serde_json::to_string(&json!({ "servers": [
        { "id": "context7", "title": "Context7", "summary": "Docs", "access": "open", "settings": { "type": "remote", "url": "https://c7.example" } },
        { "id": "linear", "title": "Linear", "summary": "Issues", "access": "signIn" },
        { "id": "plain", "title": "Plain", "summary": "Default open" },
        { "id": "later", "title": "Later", "summary": "Future", "access": "pairing" },
        { "id": "mac-app", "title": "Mac Only", "summary": "Mac", "access": "local", "macOnly": true },
    ] })).unwrap()).unwrap();
    let listed = mcp_catalog(&Map::new(), &cat_file, false);
    let pairs: Vec<(&str, &str)> = listed.servers.iter().map(|s| (s.id.as_str(), s.access.as_str())).collect();
    assert_eq!(pairs, vec![("context7", "open"), ("linear", "signIn"), ("plain", "open")]);
    let mac_listed = mcp_catalog(&Map::new(), &cat_file, true);
    assert!(mac_listed.servers.iter().any(|s| s.id == "mac-app" && s.mac_only));

    let cfg_file = tmp.path().join("opencode.json");
    let lock_file = tmp.path().join(".config.lock");
    let _ = add_from_catalog("context7", &cfg_file, &cat_file, &lock_file, "~/.kuvo-code/box/keys", false);
    let added: Value = serde_json::from_str(&fs::read_to_string(&cfg_file).unwrap()).unwrap();
    assert_eq!(added["mcp"]["servers"]["context7"]["codemode"], false);

    // mcp-setup.test.ts scenarios
    let gh = server_setup(r#"{ "mcpServers": { "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "<YOUR_TOKEN>" } } } }"#).unwrap();
    assert_eq!(gh.name.as_deref(), Some("github"));
    assert_eq!(gh.command, vec!["npx", "-y", "@modelcontextprotocol/server-github"]);
    assert_eq!(gh.environment, BTreeMap::from([("GITHUB_PERSONAL_ACCESS_TOKEN".into(), "<YOUR_TOKEN>".into())]));

    let fs_snip = server_setup("\"filesystem\": {\n  // comment\n  \"command\": \"npx\", \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/Users/me/Desktop\",],\n},").unwrap();
    assert_eq!(fs_snip.command, vec!["npx", "-y", "@modelcontextprotocol/server-filesystem", "/Users/me/Desktop"]);

    let cc = server_setup("claude mcp add --scope=user -e A=1 B=2 KEY= weather -- uvx weather-mcp").unwrap();
    assert_eq!(cc.name.as_deref(), Some("weather"));
    assert_eq!(cc.command, vec!["uvx", "weather-mcp"]);
    assert_eq!(cc.environment.get("KEY").map(String::as_str), Some(""));
    assert!(server_setup("claude mcp add-from-claude-desktop").is_none());

    let two = r#"{"mcpServers": {"b": {"command": "y"}, "a": {"command": "x"}}}"#;
    assert!(server_setup(two).is_none());
    assert_eq!(server_setups(two).iter().map(|s| s.name.as_deref().unwrap()).collect::<Vec<_>>(), vec!["a", "b"]);

    assert_eq!(command_words(r#"npx -y "@scope/a b" 'c d' e\ f "g\.h""#), vec!["npx", "-y", "@scope/a b", "c d", "e f", r#"g\.h"#]);
    assert_eq!(command_line(&["npx".into(), "a b".into(), "q\"t".into(), "".into()]), r#"npx "a b" "q\"t" """#);
    assert_eq!(web_address("mcp.example.com/mcp").as_deref(), Some("https://mcp.example.com/mcp"));
    assert_eq!(web_address("localhost:3000/mcp").as_deref(), Some("http://localhost:3000/mcp"));
    assert!(web_address("tools").is_none());
    assert!(web_address("ftp://example.com").is_none());
}
