//! Scope remembered permission decisions to the commands and files they cover.

use super::{Allowance, changes, covered, is_shell, shell};
use crate::session::PermissionAsk;

pub fn remember(ask: &PermissionAsk) -> Allowance {
    Allowance {
        action: ask.action.clone(),
        patterns: save_patterns(ask),
    }
}

pub fn covers(allowance: &Allowance, ask: &PermissionAsk) -> bool {
    (allowance.action == ask.action || changes(&allowance.action) && changes(&ask.action))
        && ask
            .resources
            .iter()
            .all(|resource| covered(&allowance.patterns, &ask.action, resource))
}

fn save_patterns(ask: &PermissionAsk) -> Vec<String> {
    if !ask.save.is_empty() {
        return ask.save.clone();
    }
    if changes(&ask.action) {
        return vec!["*".into()];
    }
    if !is_shell(&ask.action) {
        return ask.resources.clone();
    }

    let mut patterns = Vec::new();
    for line in &ask.resources {
        let line = line.trim();
        let commands = shell::commands(line);
        if !commands.known {
            push(&mut patterns, line.to_owned());
            continue;
        }
        let mut names = commands
            .parts()
            .iter()
            .map(String::as_str)
            .filter(|command| command.split_whitespace().next() != Some("cd"))
            .peekable();
        if names.peek().is_none() {
            push(&mut patterns, line.to_owned());
            continue;
        }
        let kinds = names.map(command_kind).collect::<Vec<_>>();
        if kinds.iter().any(Option::is_none) {
            push(&mut patterns, line.to_owned());
        }
        for kind in kinds.into_iter().flatten() {
            push(&mut patterns, format!("{kind} *"));
        }
    }
    patterns
}

fn push(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn command_kind(command: &str) -> Option<String> {
    let words = command.split_whitespace().collect::<Vec<_>>();
    let program = *words.first()?;
    if !program
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        || matches!(
            program,
            "sudo"
                | "doas"
                | "env"
                | "command"
                | "builtin"
                | "exec"
                | "nice"
                | "nohup"
                | "time"
                | "timeout"
                | "stdbuf"
                | "caffeinate"
        )
    {
        return None;
    }
    let count = if matches!(
        program,
        "git"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "npx"
            | "bunx"
            | "pnpx"
            | "deno"
            | "docker"
            | "podman"
            | "kubectl"
            | "helm"
            | "cargo"
            | "go"
            | "gh"
            | "brew"
            | "make"
            | "swift"
            | "uv"
            | "pip"
            | "pip3"
            | "poetry"
            | "apt"
            | "apt-get"
            | "dnf"
            | "systemctl"
            | "terraform"
            | "rustup"
            | "gcloud"
            | "aws"
            | "az"
            | "xcrun"
            | "launchctl"
            | "tmux"
            | "composer"
            | "mvn"
            | "gradle"
            | "dotnet"
            | "mix"
            | "rails"
            | "bundle"
            | "conda"
            | "flutter"
            | "dart"
            | "turbo"
            | "nx"
            | "vercel"
            | "wrangler"
            | "firebase"
            | "supabase"
    ) {
        let verb = words.get(1).filter(|name| !name.starts_with('-'))?;
        if matches!(
            (program, *verb),
            (
                "npm" | "pnpm" | "yarn" | "bun" | "uv" | "poetry" | "cargo",
                "run"
            ) | (
                "docker",
                "compose" | "container" | "image" | "volume" | "network"
            ) | ("git", "remote" | "stash" | "worktree" | "submodule")
                | ("gh", "pr" | "issue" | "repo" | "run" | "release")
                | ("aws", "s3")
                | ("kubectl", "config")
                | ("npm", "exec")
                | ("uv", "pip")
        ) {
            words.get(2).filter(|name| !name.starts_with('-'))?;
            3
        } else {
            2
        }
    } else {
        1
    };
    Some(words[..count].join(" "))
}
