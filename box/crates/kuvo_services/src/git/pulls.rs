//! Pull request drafts and creation. The branch is pushed only on the explicit Create action.
use super::{
    branches,
    changes::{self, asked, branch, read, root_of},
};
use http::StatusCode;
use kuvo_server::Refusal;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

struct Way {
    root: PathBuf,
    branch: String,
    base: Option<String>,
    against: Option<String>,
    remote: String,
}
async fn way(home: &Path, directory: &Path) -> Result<Way, Refusal> {
    let conflict = |words: String| changes::no(StatusCode::CONFLICT, words);
    let branch = branch(directory)
        .await
        .ok_or_else(|| conflict("This folder isn’t on a branch.".into()))?;
    let root = root_of(directory)
        .await
        .unwrap_or_else(|| directory.to_owned());
    let base = branches::base(home, directory, &root).await?;
    if branches::is_base(&branch, base.as_deref()) {
        return Err(conflict(format!(
            "{branch} is the branch a pull request goes into. Make one from a branch of its own."
        )));
    }
    let remote = branches::origin(&root).await.ok_or_else(|| {
        conflict(
            "This repository has no origin to push to. Add a remote called origin first.".into(),
        )
    })?;
    let mut against = None;
    if let Some(base) = &base {
        if branches::exists(&root, base).await {
            against = Some(base.clone());
        } else if branches::exists(&root, &format!("origin/{base}")).await {
            against = Some(format!("origin/{base}"));
        }
        if let Some(against) = &against
            && let Some(why) = branches::unready(directory, &branch, against).await
        {
            return Err(conflict(why.replace(&format!("origin/{base}"), base)));
        }
    }
    Ok(Way {
        root,
        branch,
        base,
        against,
        remote,
    })
}
fn page(remote: &str) -> Option<Url> {
    let remote = remote.trim();
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let converted = if let Some(rest) = remote
        .strip_prefix("ssh://")
        .or_else(|| remote.strip_prefix("git://"))
    {
        format!(
            "https://{}",
            rest.split_once('@').map_or(rest, |(_, rest)| rest)
        )
    } else if !remote.contains("://")
        && let Some((host, path)) = remote.split_once(':')
    {
        format!(
            "https://{}/{}",
            host.split_once('@').map_or(host, |(_, host)| host),
            path
        )
    } else {
        remote.to_owned()
    };
    let mut page = Url::parse(&converted).ok()?;
    if !matches!(page.scheme(), "http" | "https") {
        return None;
    }
    page.set_scheme("https").ok()?;
    page.set_username("").ok()?;
    page.set_password(None).ok()?;
    page.set_port(None).ok()?;
    Some(page)
}
fn host(page: Option<&Url>) -> &'static str {
    let host = page.and_then(Url::host_str).unwrap_or("");
    if host.contains("github") {
        "github"
    } else if host.contains("gitlab") {
        "gitlab"
    } else {
        "other"
    }
}
async fn gh(root: &Path, args: &[&str], limit: u64) -> std::io::Result<std::process::Output> {
    let mut command = tokio::process::Command::new("gh");
    command
        .args(args)
        .current_dir(root)
        .env_clear()
        .envs(crate::vault::command_environment())
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(limit), command.output())
        .await
        .map_err(std::io::Error::other)?
}
async fn summary(way: &Way) -> String {
    let range = way
        .against
        .as_ref()
        .map(|base| format!("{base}...HEAD"))
        .unwrap_or_else(|| "HEAD~1...HEAD".into());
    let commits_range = way
        .against
        .as_ref()
        .map(|base| format!("{base}..HEAD"))
        .unwrap_or_else(|| "-1".into());
    let commits = read(
        &way.root,
        &["log", "--no-color", "--format=- %s%n%b", &commits_range],
    )
    .await
    .unwrap_or_default();
    let files = read(&way.root, &["diff", "--no-color", "--stat", &range])
        .await
        .unwrap_or_default();
    let mut diff = read(&way.root, &["diff", "--no-color", "--no-ext-diff", &range])
        .await
        .unwrap_or_default();
    if diff.encode_utf16().count() > 30_000 {
        diff = String::from_utf16_lossy(&diff.encode_utf16().take(30_000).collect::<Vec<_>>());
        diff.push_str("\n…");
    }
    format!("Commits:\n{commits}\nFiles:\n{files}\nDiff:\n{diff}")
}
pub(super) async fn draft(
    home: &Path,
    directory: &Path,
    body: &Value,
    is_mac: bool,
) -> Result<Value, Refusal> {
    let way = way(home, directory).await?;
    let log_range = way
        .against
        .as_ref()
        .map(|base| format!("{base}..HEAD"))
        .unwrap_or_else(|| "-1".into());
    let listed = read(&way.root, &["log", "--no-color", "--format=%s", &log_range])
        .await
        .unwrap_or_default();
    let subjects: Vec<_> = listed.lines().filter(|s| !s.is_empty()).collect();
    let (title, description) = if subjects.len() == 1 {
        (
            subjects[0].to_owned(),
            read(&way.root, &["log", "-1", "--no-color", "--format=%b"])
                .await
                .unwrap_or_default()
                .trim()
                .to_owned(),
        )
    } else {
        let mut named = String::new();
        for c in way.branch.chars() {
            if matches!(c, '-' | '_') {
                if !named.ends_with(' ') {
                    named.push(' ');
                }
            } else {
                named.push(c);
            }
        }
        let mut chars = named.trim().chars();
        let title = chars
            .next()
            .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
            .unwrap_or_default();
        (
            title,
            subjects
                .iter()
                .rev()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };
    let page = page(&way.remote);
    let host_name = host(page.as_ref());
    let mut answer = json!({"branch":way.branch,"title":title,"body":description,"host":host_name,"creates":false});
    if let Some(base) = &way.base {
        answer["base"] = json!(base);
    }
    if host_name == "github" {
        let found = gh(
            &way.root,
            &[
                "auth",
                "status",
                "--hostname",
                page.as_ref().and_then(Url::host_str).unwrap_or(""),
            ],
            30,
        )
        .await;
        match found {
            Ok(out) if out.status.success() => answer["creates"] = json!(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                answer["gh"] = json!("missing")
            }
            _ => answer["gh"] = json!("signed-out"),
        }
    }
    if body["summary"] == true || body["prompt"] == true {
        let summary = summary(&way).await;
        if body["prompt"] == true {
            let mut prompt = format!(
                "Write a pull request for the changes below. The first line is its title: under 72 characters, in the imperative mood. Then a blank line and a short description in Markdown: what changed and why, in a few bullet points, and anything a reviewer should check. Reply with the pull request alone: no preamble, no code fences.\n\n{summary}"
            );
            let config = crate::config::ConfigPaths::for_home(home, is_mac);
            if let crate::config::SettingsRead::Ok(settings) = config.read_settings()
                && let Some(instructions) = settings
                    .get("pull_request_instructions")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                prompt.push_str("\n\nAlso follow these instructions from the user:\n");
                prompt.push_str(instructions);
            }
            answer["prompt"] = json!(prompt);
        }
        answer["summary"] = json!(summary);
    }
    Ok(answer)
}
fn compare(page: &Url, way: &Way, title: Option<&str>, body: Option<&str>) -> Option<String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    let at = page.as_str().trim_end_matches('/');
    if host(Some(page)) == "gitlab" {
        query.append_pair("merge_request[source_branch]", &way.branch);
        if let Some(base) = &way.base {
            query.append_pair("merge_request[target_branch]", base);
        }
        if let Some(title) = title {
            query.append_pair("merge_request[title]", title);
        }
        if let Some(body) = body {
            query.append_pair("merge_request[description]", body);
        }
        return Some(format!("{at}/-/merge_requests/new?{}", query.finish()));
    }
    if host(Some(page)) != "github" {
        return None;
    }
    query.append_pair("expand", "1");
    if let Some(title) = title {
        query.append_pair("title", title);
    }
    if let Some(body) = body {
        query.append_pair("body", body);
    }
    let encode = |name: &str| {
        name.split('/')
            .map(|part| {
                url::form_urlencoded::byte_serialize(part.as_bytes())
                    .collect::<String>()
                    .replace('+', "%20")
            })
            .collect::<Vec<_>>()
            .join("/")
    };
    let range = match &way.base {
        Some(base) => format!("{}...{}", encode(base), encode(&way.branch)),
        None => encode(&way.branch),
    };
    Some(format!("{at}/compare/{range}?{}", query.finish()))
}
fn pull_url(words: &str) -> Option<String> {
    words.split_whitespace().find_map(|word| {
        let (start, number) = word.split_once("/pull/")?;
        if !start.starts_with("https://") {
            return None;
        }
        let digits = number
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        (!digits.is_empty()).then(|| format!("{start}/pull/{digits}"))
    })
}
fn link(home: &Path, session: &str, url: &str) -> Option<()> {
    if !session.starts_with("ses") {
        return None;
    }
    let page = Url::parse(url).ok()?;
    let host = page.host_str()?;
    if page.scheme() != "https" || !host.contains("github") {
        return None;
    }
    let parts: Vec<_> = page.path_segments()?.collect();
    if parts.len() != 4 || parts[2] != "pull" || parts[3].parse::<i32>().ok()? <= 0 {
        return None;
    }
    let path = home.join("box/pulls.json");
    let _lock = kuvo_store::locks::StaleLock::acquire(
        &home.join("box/pulls.json.lock"),
        Duration::from_secs(10),
        Duration::from_secs(3),
        kuvo_store::locks::OnBusy::Refuse,
    )
    .ok()??;
    let kept: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut rows = kept["sessions"].as_object().cloned().unwrap_or_default();
    rows.retain(|_, v| v["url"].is_string() && v["at"].is_number());
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    rows.insert(session.into(), json!({"url":url,"at":at}));
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by_key(|(_, v)| std::cmp::Reverse(v["at"].as_u64().unwrap_or(0)));
    let latest: serde_json::Map<String, Value> = rows.into_iter().take(500).collect();
    kuvo_store::files::write_secret(
        &path,
        serde_json::to_string_pretty(&json!({"sessions":latest}))
            .ok()?
            .as_bytes(),
    )
    .ok()?;
    Some(())
}
pub(super) async fn create(home: &Path, directory: &Path, body: &Value) -> Result<Value, Refusal> {
    let way = way(home, directory).await?;
    let title = body["title"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let description = body["body"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    asked(&way.root, &["push", "-u", "origin", "HEAD"], &[])
        .await
        .map_err(|error| changes::no(StatusCode::UNPROCESSABLE_ENTITY, error))?;
    if let Some(page) = page(&way.remote) {
        if host(Some(&page)) == "github" {
            let repo = format!(
                "{}{}",
                page.host_str().unwrap_or(""),
                page.path().trim_end_matches('/')
            );
            let mut args = vec!["pr", "create", "--repo", &repo, "--head", &way.branch];
            if let Some(base) = &way.base {
                args.extend(["--base", base]);
            }
            if let Some(title) = title {
                args.extend(["--title", title, "--body", description.unwrap_or("")]);
            } else {
                args.push("--fill");
            }
            if let Ok(out) = gh(&way.root, &args, 120).await {
                let words = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                if let Some(url) = pull_url(&words) {
                    if let Some(session) = body["session"].as_str() {
                        let _ = link(home, session, &url);
                    }
                    return Ok(json!({"url":url}));
                }
            }
        }
        if let Some(compare) = compare(&page, &way, title, description) {
            return Ok(json!({"compareURL":compare}));
        }
    }
    Ok(json!({}))
}
