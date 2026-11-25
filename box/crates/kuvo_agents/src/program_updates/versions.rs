//! Reading versions and installs from what programs print and where they are.

use std::{cmp::Ordering, sync::OnceLock};

use regex::Regex;

fn pattern(cell: &'static OnceLock<Regex>, text: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(text).expect("a valid pattern"))
}

/// The first version number in some text: "2.1.156 (Claude Code)", "codex-cli 0.159.3".
pub fn version_in(text: &str) -> Option<String> {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let found = pattern(&VERSION, r"\b(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)\b").captures(text)?;
    Some(found[1].to_owned())
}

/// Comparing the numbers part by part (a pre-release before its release).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let split = |version: &str| {
        let mut pieces = version.split('-');
        let main = pieces.next().unwrap_or_default().to_owned();
        (main, pieces.next().map(str::to_owned))
    };
    let ((main_a, pre_a), (main_b, pre_b)) = (split(a), split(b));
    let parts = |main: &str| -> Vec<Option<u64>> {
        main.split('.').map(|part| part.parse().ok()).collect()
    };
    let (parts_a, parts_b) = (parts(&main_a), parts(&main_b));
    for i in 0..parts_a.len().max(parts_b.len()) {
        // (A part that isn't a number tells nothing.)
        let part = |parts: &[Option<u64>]| parts.get(i).copied().unwrap_or(Some(0));
        if let (Some(x), Some(y)) = (part(&parts_a), part(&parts_b))
            && x != y
        {
            return x.cmp(&y);
        }
    }
    match (pre_a, pre_b) {
        (a, b) if a == b => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(a), Some(b)) => a.cmp(&b),
    }
}

/// The npm prefix whose global install this is: `<prefix>/lib/node_modules/<package>/…`.
pub fn npm_global_prefix(real_path: &str, package: &str) -> Option<String> {
    static MISE: OnceLock<Regex> = OnceLock::new();
    let lower = real_path.to_ascii_lowercase();
    let segment = format!("/lib/node_modules/{}/", package.to_ascii_lowercase());
    let at = lower.rfind(&segment)?;
    if lower[..at].contains("/node_modules/") {
        return None;
    }
    // Mise's npm backend lays tools out the same way; globals of its Node are npm's.
    let mise = pattern(&MISE, r"/mise/installs/([^/]+)/[^/]+$");
    if let Some(tool) = mise.captures(&lower[..at])
        && &tool[1] != "node"
    {
        return None;
    }
    Some(if at == 0 {
        "/".into()
    } else {
        real_path[..at].to_owned()
    })
}

/// A Homebrew formula's keg (`…/Cellar/<name>/<version>/…`) or cask (`…/Caskroom/<name>/…`).
#[derive(Debug, PartialEq)]
pub struct Keg {
    pub cask: bool,
    pub name: String,
    pub prefix: String,
}

pub fn homebrew_keg(real_path: &str) -> Option<Keg> {
    static KEG: OnceLock<Regex> = OnceLock::new();
    let found = pattern(&KEG, r"(?i)^(.*)/(cellar|caskroom)/([^/]+)/[^/]+/").captures(real_path)?;
    Some(Keg {
        cask: found[2].eq_ignore_ascii_case("caskroom"),
        name: found[3].to_owned(),
        prefix: found[1].to_owned(),
    })
}

/// The newest Homebrew has, from `brew info --json=v2`.
pub(super) fn homebrew_latest(json: &str, cask: bool) -> Option<String> {
    let info: serde_json::Value = serde_json::from_str(json).ok()?;
    let raw = if cask {
        info["casks"][0]["version"].as_str()?.split(',').next()?
    } else {
        info["formulae"][0]["versions"]["stable"].as_str()?
    };
    (raw != "latest").then(|| raw.to_owned())
}

/// What a program printed for a terminal, as text.
pub(super) fn plain(text: &str) -> String {
    static RULES: OnceLock<[Regex; 6]> = OnceLock::new();
    let [forward, column, line, control, string, short] = RULES.get_or_init(|| {
        [
            r"\x1b\[(\d*)C",
            r"\x1b\[\d*G",
            // A move to another line (a terminal UI's next line) is a line break.
            r"\x1b\[[0-9;]*[BEFHfd]",
            // Any other control sequence: colors, keyboard modes and queries.
            r"\x1b\[[0-?]*[ -/]*[@-~]",
            // Strings: a title, a link, a query's answer.
            r"\x1b[\]P_^X][^\x07\x1b]*(?:\x07|\x1b\\)",
            // A character set (`ESC(B`), and the rest of the short ones.
            r"\x1b(?:[ -/]+[0-~]|[0-~])",
        ]
        .map(|rule| Regex::new(rule).expect("a valid pattern"))
    });
    let text = forward.replace_all(text, |found: &regex::Captures| {
        let count = if found[1].is_empty() {
            1
        } else {
            found[1].parse().unwrap_or(80)
        };
        " ".repeat(usize::min(count, 80))
    });
    let text = column.replace_all(&text, " ");
    let text = line.replace_all(&text, "\n");
    let text = control.replace_all(&text, "");
    let text = string.replace_all(&text, "");
    short.replace_all(&text, "").into_owned()
}
