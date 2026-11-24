//! This test binary stands in for Bun: it records how it was run and
//! installs (or fails) as the scenario's environment says.

use kuvo_agents::runtime_packages::{Installer, Package, RuntimePackages};
use kuvo_os::platform::Environment;
use std::{ffi::OsString, fs, path::Path};

#[test]
#[ignore = "run only as the stand-in installer of a scenario"]
fn stand_in_bun() {
    let folder = std::env::current_dir().unwrap();
    let args = std::env::args().collect::<Vec<_>>();
    let ours = args.iter().position(|arg| arg == "--").map_or(&args[..], |at| &args[at + 1..]);
    let record = serde_json::json!({
        "args": ours,
        "beBun": std::env::var("BUN_BE_BUN").ok(),
        "install": std::env::var("BUN_INSTALL").ok(),
        "cache": std::env::var("BUN_INSTALL_CACHE_DIR").ok(),
        "kept": std::env::var("KEPT").ok(),
        "manifest": serde_json::from_slice::<serde_json::Value>(
            &fs::read(folder.join("package.json")).unwrap()).unwrap(),
    });
    let log = std::env::var("STAND_IN_LOG").unwrap();
    let mut runs: Vec<serde_json::Value> = fs::read(&log)
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .unwrap_or_default();
    runs.push(record);
    fs::write(&log, serde_json::to_vec(&runs).unwrap()).unwrap();
    let cache = std::env::var("BUN_INSTALL_CACHE_DIR").unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(Path::new(&cache).join("downloaded"), "").unwrap();
    if std::env::var("STAND_IN_FAIL").is_ok() {
        eprintln!("error: GET https://registry.npmjs.org/@cursor%2fsdk - 404");
        std::process::exit(1);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(folder.join("package.json")).unwrap()).unwrap();
    for (name, version) in manifest["dependencies"].as_object().unwrap() {
        let package = folder.join("node_modules").join(name);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            serde_json::json!({ "name": name, "version": version }).to_string(),
        )
        .unwrap();
    }
}

fn packages(root: &Path, extra: &[(&str, &str)]) -> RuntimePackages {
    let mut env: Environment = std::env::vars_os()
        .filter(|(name, _)| {
            ["SystemRoot", "WINDIR", "PATH"]
                .iter()
                .any(|key| name.eq_ignore_ascii_case(key))
        })
        .collect();
    env.insert("KEPT".into(), "yes".into());
    env.insert("STAND_IN_LOG".into(), root.join("runs.json").into());
    for (name, value) in extra {
        env.insert((*name).into(), (*value).into());
    }
    RuntimePackages::new(
        root.join("runtime"),
        root.join("bun"),
        Installer {
            program: std::env::current_exe().unwrap(),
            leading: ["--ignored", "--exact", "--nocapture", "--quiet", "stand_in_bun", "--"]
                .map(OsString::from)
                .to_vec(),
            env,
        },
    )
}

fn sdk(version: &str) -> Vec<Package> {
    vec![Package {
        name: "@cursor/sdk".into(),
        version: version.into(),
    }]
}

fn runs(root: &Path) -> Vec<serde_json::Value> {
    serde_json::from_slice(&fs::read(root.join("runs.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn an_agents_packages_install_once_in_its_own_folder() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let packages = packages(root, &[]);
    assert!(!packages.has("cursor", &sdk("1.0.35")));
    let wanted = sdk("1.0.35");
    let (first, second) = tokio::join!(
        packages.ensure("cursor", &wanted, true),
        packages.ensure("cursor", &wanted, false),
    );
    let folder = first.unwrap();
    assert_eq!(second.unwrap(), folder);
    assert_eq!(folder, root.join("runtime/cursor"));
    assert!(packages.has("cursor", &sdk("1.0.35")));
    // Two callers at once: one install.
    let runs = runs(root);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(
        run["args"],
        serde_json::json!(["install", "--no-save", "--no-progress", "--silent"])
    );
    assert_eq!(run["beBun"], "1");
    assert_eq!(run["kept"], "yes");
    assert_eq!(run["install"], root.join("bun").to_str().unwrap());
    assert_eq!(run["cache"], root.join("bun/install").to_str().unwrap());
    assert_eq!(
        run["manifest"],
        serde_json::json!({"name":"kuvo-agents-runtime","private":true,
            "dependencies":{"@cursor/sdk":"1.0.35"}})
    );
    assert_eq!(
        fs::read_to_string(folder.join("package.json")).unwrap(),
        "{\n  \"name\": \"kuvo-agents-runtime\",\n  \"private\": true,\n  \"dependencies\": {\n    \"@cursor/sdk\": \"1.0.35\"\n  }\n}"
    );
    // The download cache goes once it's installed.
    assert!(!root.join("bun/install").exists());
    // Only the agent's folder is left beside it.
    let names = fs::read_dir(root.join("runtime"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [OsString::from("cursor")]);

    // A new version replaces the folder; without optional packages it says so.
    packages
        .ensure("cursor", &sdk("1.0.36"), false)
        .await
        .unwrap();
    assert!(packages.has("cursor", &sdk("1.0.36")));
    assert_eq!(
        self::runs(root)[1]["args"],
        serde_json::json!(["install", "--no-save", "--no-progress", "--silent", "--omit", "optional"])
    );
    packages.reset("cursor").unwrap();
    assert!(!root.join("runtime/cursor").exists());
    packages.reset("cursor").unwrap();
}

#[tokio::test]
async fn a_failed_install_says_why_and_leaves_the_last_one() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    packages(root, &[])
        .ensure("cursor", &sdk("1.0.35"), true)
        .await
        .unwrap();
    // What an interrupted install left is cleared before the next.
    fs::create_dir_all(root.join("runtime/.cursor-installing-old/node_modules")).unwrap();
    let failing = packages(root, &[("STAND_IN_FAIL", "1")]);
    let error = failing
        .ensure("cursor", &sdk("1.0.36"), true)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Couldn't download what the agent needs (@cursor/sdk): error: GET https://registry.npmjs.org/@cursor%2fsdk - 404"
    );
    assert!(failing.has("cursor", &sdk("1.0.35")));
    assert!(!root.join("bun/install").exists());
    let names = fs::read_dir(root.join("runtime"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [OsString::from("cursor")]);
}
