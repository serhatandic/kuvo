//! What a notice shows (the same rules as the clients' secret mask, held to
//! `box/test/fixtures/masking.json`): what looks like a secret (a key, a token, a password) as
//! dots, everything else as written, so a notice never shows one and still says what happened.

use std::sync::LazyLock;

use regex::{Captures, Regex};

const SHOWN_LENGTH: usize = 4000;
const DOTS: &str = "•••";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a mask's pattern")
}

macro_rules! pattern {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| re($pattern));
    };
}

const fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Each match of `pattern` that `keeps` (looking at the text around it) replaced `with`; one it
/// doesn't is looked for again a character on, as a pattern's own look-arounds would.
fn replaced(
    text: &str,
    pattern: &Regex,
    keeps: impl Fn(&str, &Captures) -> bool,
    with: impl Fn(&Captures) -> String,
) -> String {
    let (mut out, mut last, mut from) = (String::new(), 0, 0);
    while from <= text.len() {
        let Some(found) = pattern.captures_at(text, from) else {
            break;
        };
        let whole = found.get(0).expect("a match");
        if !keeps(text, &found) {
            from = whole.start()
                + text[whole.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            continue;
        }
        out.push_str(&text[last..whole.start()]);
        out.push_str(&with(&found));
        last = whole.end();
        from = whole.end().max(whole.start() + 1);
    }
    out.push_str(&text[last.min(text.len())..]);
    out
}

fn before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

pattern!(TRIMMED, r#"[\s:=>"'\]\\_-]+$"#);
pattern!(SPLIT_NAME, r#"[\s(\[{,;"'=/]+"#);
pattern!(SECRET_WORD, r"(?i)secret|private|pass|pwd|token|credential");
pattern!(KEY_END, r"(?i)key$");
pattern!(NOT_SECRET_KEY, r"(?i)(?:cache|public)[ _.-]?key$");
pattern!(
    HASH_NAME,
    r"(?i)(?:commit|sha|sha1|sha256|sha512|md5|rev|revision|hash|ref|head|onto|checksum|digest|etag|integrity|fingerprint|cache[ _.-]?key|public[ _.-]?key|object|oid|tree|parent|blob|base|id|uuid|tag|build|version|release)s?$"
);

fn trimmed_name(prefix: &str) -> &str {
    TRIMMED
        .find(prefix)
        .map_or(prefix, |found| &prefix[..found.start()])
}

pub fn is_secret_name(prefix: &str) -> bool {
    let name = trimmed_name(prefix);
    let word = SPLIT_NAME
        .split(name)
        .filter(|each| !each.is_empty())
        .last()
        .unwrap_or_default();
    SECRET_WORD.is_match(word) || (KEY_END.is_match(name) && !NOT_SECRET_KEY.is_match(name))
}

pub fn names_hash(prefix: &str) -> bool {
    if let Some(before) = prefix.strip_suffix(':') {
        let token = before.split_whitespace().last();
        if token.is_some_and(|token| token.contains(['/', '.']))
            && before
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
        {
            return true;
        }
    }
    !is_secret_name(prefix) && HASH_NAME.is_match(trimmed_name(prefix))
}

/// The (up to) `count` characters before byte `at`.
fn chars_before(text: &str, at: usize, count: usize) -> &str {
    let start = text[..at]
        .char_indices()
        .rev()
        .nth(count - 1)
        .map_or(0, |(start, _)| start);
    &text[start..at]
}

pattern!(
    HEX_AFTER_NAME,
    r#"(?:\\?["'])?\]?[ \t]*(?::=|=>|[:=])[ \t]*(?:\\?["'])?([0-9a-fA-F]{32,})"#
);

/// A long hex value after a name that isn't a hash's: dots.
fn masked_hex_values(text: &str) -> String {
    replaced(
        text,
        &HEX_AFTER_NAME,
        |text, found| {
            let (whole, hex) = (
                found.get(0).expect("a match"),
                found.get(1).expect("its hex"),
            );
            before(text, whole.start()).is_some_and(word)
                && text[hex.end()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric())
                && !names_hash(chars_before(text, hex.start(), 32))
        },
        |found| {
            let (whole, hex) = (
                found.get(0).expect("a match"),
                found.get(1).expect("its hex"),
            );
            let text = whole.as_str();
            let at = hex.start() - whole.start();
            format!("{}{DOTS}", &text[..at])
        },
    )
}

/// A run's hashes (a commit's, one a name says is a hash) marked as such: they're no secret.
fn marking_hashes(run: &str, before_run: &str) -> String {
    let chars: Vec<(usize, char)> = run.char_indices().collect();
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    let (mut out, mut at) = (String::new(), 0);
    while at < chars.len() {
        let (start, c) = chars[at];
        let fresh = at
            .checked_sub(1)
            .is_none_or(|prev| !chars[prev].1.is_ascii_alphanumeric());
        let len = chars[at..].iter().take_while(|(_, c)| hex(*c)).count();
        let after = chars.get(at + len).map(|(_, c)| *c);
        if !hex(c) || !fresh || len < 32 || after.is_some_and(|c| c.is_ascii_alphanumeric()) {
            out.push(c);
            at += 1;
            continue;
        }
        let prefix_source = format!("{before_run}{}", &run[..start]);
        let prefix = chars_before(&prefix_source, prefix_source.len(), 32);
        let commit = len == 40 || len == 64;
        let last = prefix.chars().next_back();
        let is_hash = if commit && last.is_none_or(|c| c.is_whitespace() || "/+\"'([".contains(c)) {
            true
        } else if commit && prefix.ends_with('-') {
            !is_secret_name(prefix)
        } else {
            names_hash(prefix)
        };
        if is_hash {
            out.push('h');
        } else {
            out.extend(chars[at..at + len].iter().map(|(_, c)| *c));
        }
        at += len;
    }
    out
}

/// How many words a run has, as a camel-cased name splits: "APIKey" is two.
fn words_in(part: &str) -> usize {
    let chars: Vec<char> = part.chars().collect();
    let (mut count, mut at) = (0, 0);
    while at < chars.len() {
        let c = chars[at];
        let run_of =
            |from: usize, of: fn(&char) -> bool| chars[from..].iter().take_while(|c| of(c)).count();
        let len = if c.is_ascii_uppercase() {
            let upper = run_of(at, char::is_ascii_uppercase);
            let lower_after = chars.get(at + upper).is_some_and(char::is_ascii_lowercase);
            if upper >= 2 && lower_after {
                upper - 1
            } else if chars.get(at + 1).is_some_and(char::is_ascii_lowercase) {
                1 + run_of(at + 1, char::is_ascii_lowercase)
            } else {
                upper
            }
        } else if c.is_ascii_lowercase() {
            run_of(at, char::is_ascii_lowercase)
        } else if c.is_ascii_digit() {
            run_of(at, char::is_ascii_digit)
        } else {
            at += 1;
            continue;
        };
        count += 1;
        at += len;
    }
    count
}

/// `text` cut to what a notice shows, between words, so a key isn't cut short of looking like one.
fn cut_shown(text: &str) -> String {
    let start: Vec<char> = text.chars().take(SHOWN_LENGTH).collect();
    let keeps = |c: char| c.is_alphanumeric() || "_-+/=".contains(c);
    let low = start.len().saturating_sub(100);
    let end = (low..start.len())
        .rev()
        .find(|at| !keeps(start[*at]))
        .unwrap_or(start.len());
    format!("{}…", start[..end].iter().collect::<String>())
}

pattern!(
    NAMED,
    r#"(?i)((?:api[_-]?key|secret(?:[_-]?access[_-]?key)?|password|passwd|authorization)["']?\s*[:=]\s*["']?(?:bearer\s+|basic\s+|token\s+(?:token=)?["']?)?|bearer\s+|--(?:[A-Za-z0-9_][A-Za-z0-9_-]{0,59}?)??(?:password|passwd|secret)[=\s]+)([^\s"'&;]{3,})"#
);
pattern!(
    OPTIONAL_SCHEME,
    r#"(?i)^(?:bearer\s+|basic\s+|token\s+(?:token=)?["']?)"#
);
pattern!(FLAG_START, r"--[A-Za-z0-9_]");
pattern!(FLAG_KEY, r"(?i)^(?:token|key)[=\s]+");
pattern!(
    FLAG_NOT_BEFORE,
    r"(?i)(?:(?:sort|foreign|partition|group|cache|order|index|range|lookup|hash)[-_]|hot|turn|mon)$"
);
pattern!(VALUE, r#"^[^\s"'&;]{3,}"#);
pattern!(SHORT_NUMBER, r#"^[0-9][^\s"'&;]{0,7}(?:[\s"'&;]|$)"#);
pattern!(
    TOKEN,
    r#"(?i)(token["']?\s*[:=]\s*["']?(?:bearer\s+|basic\s+)?)([^\s"'&;]{3,})"#
);
pattern!(TOKEN_SCHEME, r"(?i)^(?:bearer\s+|basic\s+)");
pattern!(NUMBER_VALUE, r#"^[0-9]+(?:[\s"'&;,}]|$)"#);
pattern!(URL_PASSWORD, r"(://[^\s/:@]+:)[^\s/@]+@");
pattern!(URL_USER, r"(://)[^\s/:@]{20,}@");
pattern!(
    CURL_USER,
    r"((?-u:\b)curl(?-u:\b)[^\n]*?\s(?:-u|--user)\s+[^\s:]+:)\S+"
);
pattern!(ACCESS_KEY, r"(?i)(secret[_-]?access[_-]?key\s+)\S+");
pattern!(SLACK, r#"(hooks\.slack\.com/services/)[^\s"'<>]+"#);
pattern!(
    MYSQL,
    r"((?-u:\b)(?i:mysql|mariadb|mysqldump|mysqladmin)(?-u:\b)[^\n]*?\s-p)\S+"
);

/// `--…token value` and `--…key value` (not a sort key, a hotkey…): the value as dots.
fn masked_flags(text: &str) -> String {
    let (mut out, mut last, mut from) = (String::new(), 0, 0);
    while let Some(flag) = FLAG_START.find_at(text, from) {
        from = flag.start() + 1;
        if before(text, flag.start()).is_some_and(|c| word(c) || c == '-') {
            continue;
        }
        // The first token or key in its name (at most sixty characters in) that works.
        let name_start = flag.start() + 2;
        let run = text[name_start..]
            .bytes()
            .take(60)
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
            .count();
        let found = (0..=run).find_map(|skip| {
            let at = name_start + skip;
            let key = FLAG_KEY.find(&text[at..])?;
            if FLAG_NOT_BEFORE.is_match(&text[flag.start()..at]) {
                return None;
            }
            let value_at = at + key.end();
            let rest = &text[value_at..];
            if rest.starts_with('-') || SHORT_NUMBER.is_match(rest) {
                return None;
            }
            VALUE
                .find(rest)
                .map(|value| (value_at, value_at + value.end()))
        });
        if let Some((value_at, end)) = found {
            out.push_str(&text[last..value_at]);
            out.push_str(DOTS);
            last = end;
            from = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

pub fn masked_named(text: &str) -> String {
    // (The scheme a value has is taken whole: "Bearer" is never the value.)
    let named = replaced(
        text,
        &NAMED,
        |text, found| {
            let (whole, prefix) = (
                found.get(0).expect("a match"),
                found.get(1).expect("its name"),
            );
            let after = &text[prefix.end()..];
            let flag = prefix.as_str().starts_with("--");
            if flag && before(text, whole.start()).is_some_and(|c| word(c) || c == '-') {
                return false;
            }
            if flag && after.starts_with('-') {
                return false;
            }
            !(found
                .get(1)
                .is_some_and(|name| !name.as_str().to_ascii_lowercase().starts_with("bearer"))
                && !flag
                && OPTIONAL_SCHEME.is_match(after))
        },
        |found| format!("{}{DOTS}", &found[1]),
    );
    let text = masked_flags(&named);
    let text = masked_hex_values(&text);
    let text = replaced(
        &text,
        &TOKEN,
        |text, found| {
            let prefix = found.get(1).expect("its name");
            let after = &text[prefix.end()..];
            !TOKEN_SCHEME.is_match(after) && !NUMBER_VALUE.is_match(after)
        },
        |found| format!("{}{DOTS}", &found[1]),
    );
    let text = URL_PASSWORD.replace_all(&text, "${1}•••@");
    let text = URL_USER.replace_all(&text, "${1}•••@");
    let text = CURL_USER.replace_all(&text, "${1}•••");
    let text = ACCESS_KEY.replace_all(&text, "${1}•••");
    let text = SLACK.replace_all(&text, "${1}•••");
    MYSQL.replace_all(&text, "${1}•••").into_owned()
}

pattern!(RUN, r"[A-Za-z0-9_\-+/=]{20,}");
pattern!(AWS_KEY, r"^(?:AKIA|ASIA)[0-9A-Z]{16}$");
pattern!(
    KEY_STARTS,
    r"^(?:sk-|sk_live_|sk_test_|rk_live_|rk_test_|whsec_|ghp_|gho_|ghs_|github_pat_|xox[abpe]-|AKIA|AIza|glpat-|dop_v1_|sbp_|pul-|npm_|hf_|gsk_|xai-|pplx-|r8_|lin_api_|ntn_|shp(?:at|ss|ca)_)"
);
pattern!(
    IS_HASH,
    r"^(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64}|[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$"
);
pattern!(
    UUID,
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
);
pattern!(TEMP_FOLDER, r"/var/folders/[^/]+/[^/]+");
pattern!(HEX_WORD, r"^[0-9]*[a-fA-F][0-9a-fA-F]*$");

/// `text` as a notice shows it: what looks like a secret as dots.
pub fn masked(text: &str) -> String {
    let text = if text.chars().count() > SHOWN_LENGTH {
        cut_shown(text)
    } else {
        text.to_owned()
    };
    let text = masked_named(&text);
    let (mut result, mut at) = (String::new(), 0);
    for found in RUN.find_iter(&text) {
        let (run, location) = (found.as_str(), found.start());
        if run.len() < 24 {
            if AWS_KEY.is_match(run) {
                result.push_str(&text[at..location]);
                result.push_str(DOTS);
                at = found.end();
            }
            continue;
        }
        let prefixed = run.split('/').any(|part| KEY_STARTS.is_match(part));
        let is_hash = IS_HASH.is_match(run);
        let hashes = marking_hashes(run, chars_before(&text, location, 32));
        let hashes = replaced(
            &hashes,
            &UUID,
            |text, found| {
                let whole = found.get(0).expect("a match");
                before(text, whole.start()).is_none_or(|c| !c.is_ascii_hexdigit())
                    && text[whole.end()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_ascii_hexdigit())
            },
            |_| "u".to_owned(),
        );
        let plain = TEMP_FOLDER.replace_all(&hashes, "/var/folders/t/t");
        let wordy = plain
            .split(['/', '-', '_', '.'])
            .filter(|part| !part.is_empty())
            .all(|part| {
                if part.len() < 12 {
                    return true;
                }
                if HEX_WORD.is_match(part) {
                    return part.len() < 20;
                }
                part.len() as f64 / words_in(part).max(1) as f64 >= 3.5
            });
        let mixed =
            run.chars().any(|c| c.is_ascii_alphabetic()) && run.chars().any(|c| c.is_ascii_digit());
        if !((prefixed && mixed) || (mixed && !is_hash && !wordy)) {
            continue;
        }
        result.push_str(&text[at..location]);
        result.push_str(DOTS);
        at = found.end();
    }
    result.push_str(&text[at..]);
    result
}

/// Whether a notice shows all of `text`: one that hides some of it doesn't answer from its
/// buttons, as you'd answer what you can't see.
pub fn shows_all(text: &str) -> bool {
    text.chars().count() <= SHOWN_LENGTH && masked(text) == text
}

#[cfg(test)]
mod tests {
    use super::{masked, shows_all};

    #[test]
    fn the_masking_fixture_is_masked_as_the_clients_mask_it() {
        let fixture = include_str!("../../../../test/fixtures/masking.json");
        let fixture: serde_json::Value = serde_json::from_str(fixture).expect("the fixture");
        let mut wrong = Vec::new();
        for case in fixture["cases"].as_array().expect("its cases") {
            let text = case["text"].as_str().expect("a text");
            let (got, all) = (masked(text), shows_all(text));
            if got != case["masked"] || all != case["showsAll"] {
                wrong.push(format!(
                    "{text:?}\n  got  {got:?} {all}\n  want {} {}",
                    case["masked"], case["showsAll"]
                ));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
