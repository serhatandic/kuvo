//! A notice in plain words, kept short (PROTOCOL.md, "Notifications"): a reply's first sentence,
//! its Markdown taken off; what's asked in a few words, never a command line, a path, a flag or a
//! code (those belong on the card in the conversation, and in logs).

use std::sync::LazyLock;

use regex::Regex;

use super::Notice;

/// The longest a notice's sentence is, in characters: what a lock screen shows in two lines or so.
pub const SENTENCE_LONGEST: usize = 100;
/// The longest a line the box words itself is ("Wants to run du."): a short one.
pub const LINE_LONGEST: usize = 60;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a notice's pattern")
}

macro_rules! pattern {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| re($pattern));
    };
}

pattern!(LINE_MARK, r"(?m)^[ \t]{0,3}(?:#{1,6}|>+)[ \t]+");
pattern!(LINK, r"!?\[([^\]]*)\]\([^)\s]*\)");
pattern!(SPACES, r"\s+");
pattern!(
    ABBREVIATION,
    r"(?i)(?:^|\s)(?:e\.g|i\.e|vs|cf|approx|incl|mr|mrs|ms|dr|fig)\.$"
);
pattern!(
    WEB,
    r#"(?i)(?-u:\b)https?://(?:www\.)?([^\s/?#)\]>"']+)[^\s)\]>"']*"#
);
pattern!(
    ABSOLUTE,
    r"^(?:~|\.{1,2})?/(?:[A-Za-z0-9_.@+-]+/)+[A-Za-z0-9_.@+-]*"
);
pattern!(RELATIVE, r"^[A-Za-z0-9_.@+-]+(?:/[A-Za-z0-9_.@+-]+){2,}");
pattern!(FENCE, r"(?s)```.*?(?:```|\z)");
pattern!(HEADING, r"^#{1,6}\s");
pattern!(HEADING_MARKS, r"^#{1,6}\s+");
pattern!(HTML_LINE, r"(?i)^</?[a-z][^>]*>$");
pattern!(ITEM, r"^(?:[-*+]|[0-9]+[.)])\s+(.*)$");
pattern!(PROGRAM, r"^[A-Za-z][A-Za-z0-9_.+-]{0,24}$");
pattern!(PARTS, r"\s*(?:&&|\|\||;)\s*");
pattern!(SETTING, r"^[A-Za-z_][A-Za-z0-9_]*=");
pattern!(FILE_NAME, r"^[A-Za-z0-9_][A-Za-z0-9_ .+@()-]{0,38}$");
pattern!(HOST, r"(?i)^https?://(?:www\.)?([^\s/?#:]+)");
pattern!(
    CODE,
    r"[{}\[\]<>/\\]|(?-u:\b)[A-Z][A-Z0-9_]{3,}(?-u:\b)|(?-u:\b)[A-Za-z0-9_]*[0-9]{3,}[A-Za-z0-9_]*(?-u:\b)|[A-Za-z0-9_]+_[A-Za-z0-9_]+|(?-u:\b)[A-Za-z0-9_]+\.[A-Za-z0-9_]+\(|0x[0-9a-fA-F]+"
);

const fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Emphasis's marks: a run of up to three `*` or `_` that opens words (nothing wordy before it,
/// something after) or closes them (something before it, nothing wordy after).
fn without_emphasis(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mark = |at: usize| chars.get(at).is_some_and(|c| *c == '*' || *c == '_');
    let (mut kept, mut at) = (String::new(), 0);
    while at < chars.len() {
        let run = (0..3).take_while(|step| mark(at + step)).count();
        let before = at.checked_sub(1).map(|at| chars[at]);
        let opens = |len: usize| chars.get(at + len).is_some_and(|c| !c.is_whitespace());
        let closes = |len: usize| chars.get(at + len).is_none_or(|c| !word(*c) && *c != '*');
        let open = before.is_none_or(|c| !word(c) && c != '*');
        let close = before.is_some_and(|c| !c.is_whitespace());
        let len = (1..=run)
            .rev()
            .find(|len| open && opens(*len))
            .or_else(|| (1..=run).rev().find(|len| close && closes(*len)));
        match len {
            Some(len) => at += len,
            None => {
                kept.push(chars[at]);
                at += 1;
            }
        }
    }
    kept
}

/// Markdown's marks taken off, not the words: headings and quotes at a line's start, emphasis
/// around words, code ticks, a link's address.
pub fn plain(text: &str) -> String {
    let text = LINE_MARK.replace_all(text, "");
    let text = LINK.replace_all(&text, "$1");
    let text = without_emphasis(&text).replace('`', "");
    SPACES.replace_all(&text, " ").trim().to_owned()
}

/// The first sentence of a run of plain words: up to the first ".", "!", "?" or "…" with a space
/// or the end after it (not a version's dot, "v1.2", nor an abbreviation's).
fn first_of(words: &str) -> &str {
    let ends = |c: char| ".!?…".contains(c);
    let quotes = |c: char| "\"”’)]".contains(c);
    let mut indices = words.char_indices().peekable();
    while let Some((at, c)) = indices.next() {
        if !ends(c) {
            continue;
        }
        let mut end = at + c.len_utf8();
        let rest = &words[end..];
        let run: usize = rest
            .chars()
            .take_while(|c| ends(*c))
            .map(char::len_utf8)
            .sum();
        end += run;
        end += words[end..]
            .chars()
            .take_while(|c| quotes(*c))
            .map(char::len_utf8)
            .sum::<usize>();
        let spaced = words[end..].chars().next().is_none_or(char::is_whitespace);
        if spaced && !ABBREVIATION.is_match(&words[..=at]) {
            return &words[..end];
        }
        // (The run's own marks start no other sentence.)
        while indices.peek().is_some_and(|(at, _)| *at < end && spaced) {
            indices.next();
        }
    }
    words
}

/// Cut at a word, ending in "…", to `longest` characters (never through one, an emoji's say).
pub fn cut_at_word(text: &str, longest: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= longest {
        return text.to_owned();
    }
    let room: String = chars[..longest - 1].iter().collect();
    let space = room.chars().rev().position(|c| c == ' ');
    let room = match space.map(|from_end| longest - 2 - from_end) {
        Some(at) if at > longest / 2 => room.chars().take(at).collect(),
        _ => room,
    };
    let trimmed = room.trim_end_matches(|c: char| c.is_whitespace() || ",;:–—-".contains(c));
    format!("{trimmed}…")
}

/// `text` cut to `count` characters, ending in "…".
pub fn cut(text: &str, count: usize) -> String {
    if text.encode_utf16().count() <= count {
        return text.to_owned();
    }
    let kept: String = text.chars().take(count.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// A word of an agent's that is an address, said as people know it: a web address as its site
/// ("github.com"), a path as its last name ("notes.md").
pub fn plain_addresses(text: &str) -> String {
    let text = WEB.replace_all(text, "$1");
    let last_name = |address: &str| {
        let trimmed = address.trim_end_matches('/');
        let last = trimmed.rsplit('/').next().unwrap_or_default();
        if last.is_empty() { address } else { last }.to_owned()
    };
    // A relative one of three names or more whose last has a letter (not a date): fewer names at
    // its end where that's what it takes.
    let relative = |found: &str| {
        let mut path = found;
        loop {
            let last = path.rsplit('/').next().unwrap_or_default();
            if last.chars().any(|c| c.is_ascii_alphabetic()) {
                return Some(path.len());
            }
            if path.matches('/').count() <= 2 {
                return None;
            }
            path = &path[..path.len() - last.len() - 1];
        }
    };
    let (mut kept, mut at) = (String::new(), 0);
    while at < text.len() {
        let before = text[..at].chars().next_back();
        let starts = before.is_none_or(|c| !(word(c) || ".:/~@-".contains(c)));
        let rest = &text[at..];
        let found = starts
            .then(|| {
                ABSOLUTE.find(rest).map(|found| found.len()).or_else(|| {
                    RELATIVE
                        .find(rest)
                        .and_then(|found| relative(found.as_str()))
                })
            })
            .flatten();
        if let Some(len) = found {
            kept.push_str(&last_name(&rest[..len]));
            at += len;
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        kept.push(c);
        at += c.len_utf8();
    }
    kept
}

/// Whether a line is a rule (`---`, `* * *`).
fn rule(line: &str) -> bool {
    let marks: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && "-*_".contains(marks[0]) && marks.iter().all(|c| *c == marks[0])
}

/// Whether a line is all in bold, as a heading's often written (`**Done**`, `__Next:__`).
fn bold_line(line: &str) -> bool {
    ["**", "__"].iter().any(|mark| {
        let Some(inner) = line.strip_prefix(mark) else {
            return false;
        };
        let inner = inner.strip_suffix(':').unwrap_or(inner);
        inner
            .strip_suffix(mark)
            .is_some_and(|words| !words.is_empty() && !words.contains(['*', '_']))
    })
}

/// What `text` (an agent's reply, in Markdown) says first, as one sentence of plain words: its
/// first paragraph's first sentence, past headings, code, tables and rules; a line that leads into
/// a list ("Done:") with the list's first item. Cut at a word to `longest`; "" for nothing to say.
pub fn one_sentence(text: &str, longest: usize) -> String {
    let text = FENCE
        .replace_all(&plain_addresses(text), "\n\n")
        .into_owned();
    let (mut paragraphs, mut current, mut heading) = (Vec::new(), Vec::<String>::new(), None);
    let close = |current: &mut Vec<String>, paragraphs: &mut Vec<String>| {
        if !current.is_empty() {
            paragraphs.push(plain(&current.join(" ")));
        }
        current.clear();
    };
    for raw in text.split('\n') {
        let line = raw.trim();
        if line.is_empty()
            || HEADING.is_match(line)
            || line.starts_with('|')
            || rule(line)
            || bold_line(line)
            || HTML_LINE.is_match(line)
        {
            close(&mut current, &mut paragraphs);
            // (A heading says it, where nothing else does: "**Done.**" alone.)
            if heading.is_none()
                && (HEADING.is_match(line) || line.starts_with("**") || line.starts_with("__"))
            {
                heading = Some(plain(&HEADING_MARKS.replace(line, "")));
            }
            continue;
        }
        if let Some(item) = ITEM.captures(line) {
            close(&mut current, &mut paragraphs);
            current.push(item[1].to_owned());
            close(&mut current, &mut paragraphs);
            continue;
        }
        current.push(line.to_owned());
    }
    close(&mut current, &mut paragraphs);
    let found: Vec<&String> = paragraphs.iter().filter(|each| !each.is_empty()).collect();
    let Some(first) = found.first() else {
        return heading.map_or_else(String::new, |heading| {
            cut_at_word(first_of(&heading), longest)
        });
    };
    let mut sentence = first_of(first).to_owned();
    // "Here's what changed:" says nothing alone: with what comes first after it.
    if sentence == **first
        && sentence.ends_with(':')
        && let Some(next) = found.get(1)
    {
        sentence = format!("{sentence} {}", first_of(next));
    }
    cut_at_word(&sentence, longest)
}

/// What a command line runs first in each of its parts, past a `cd` and settings before it.
fn programs_of(line: &str) -> Vec<String> {
    PARTS
        .split(line)
        .filter_map(|part| {
            let words = part.split_whitespace();
            let mut words = words.skip_while(|word| SETTING.is_match(word));
            let name = words.next()?.rsplit('/').next()?.to_owned();
            (PROGRAM.is_match(&name) && name != "cd").then_some(name)
        })
        .collect()
}

/// What a command asks to run, in plain words: "a command", "du", "du and git". `whole`: the
/// words say all it covers (each line is a program's name alone).
pub fn command_words(lines: &[String]) -> (String, bool) {
    let specific: Vec<&String> = lines
        .iter()
        .filter(|line| !line.trim().is_empty() && line.trim() != "*")
        .collect();
    let mut names: Vec<String> = Vec::new();
    for name in specific.iter().flat_map(|line| programs_of(line)) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let words = match names.len() {
        0 => return ("a command".to_owned(), specific.is_empty()),
        1 | 2 => names.join(" and "),
        more => format!("{}, {} and {} more", names[0], names[1], more - 2),
    };
    let whole = names.len() <= 2 && specific.iter().all(|line| PROGRAM.is_match(line.trim()));
    (words, whole)
}

/// A permission, asked of you, in a short plain line ("Wants to run du."): what it would do,
/// naming at most a program, a file's name or a site. `whole`: the line says all it covers, so a
/// notification's buttons may answer it.
pub fn permission_line(action: &str, resources: &[String]) -> (String, bool) {
    let resources: Vec<&String> = resources.iter().filter(|each| *each != "*").collect();
    let none = resources.is_empty();
    let say = |words: &str, whole: bool| {
        (
            cut_at_word(&format!("Wants to {words}."), LINE_LONGEST),
            whole,
        )
    };
    let files = |verb: &str, one: &str, many: &str| {
        let name = |resource: &str| {
            let name = resource
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default();
            FILE_NAME.is_match(name).then(|| name.to_owned())
        };
        let only = (resources.len() == 1).then(|| name(resources[0])).flatten();
        let words = match (none, only) {
            (true, _) => one.to_owned(),
            (_, Some(only)) => format!("{verb} {only}"),
            _ if resources.len() > 1 => format!("{verb} {} {many}", resources.len()),
            _ => one.to_owned(),
        };
        say(&words, none)
    };
    match action {
        "edit" | "write" | "patch" => files("edit", "edit files", "files"),
        "read" => files("read", "read a file", "files"),
        "shell" | "bash" => {
            let lines: Vec<String> = resources.iter().map(|each| (*each).clone()).collect();
            let (words, whole) = command_words(&lines);
            say(&format!("run {words}"), whole)
        }
        "external_directory" => say("work outside the project", none),
        "webfetch" => {
            let mut hosts: Vec<Option<String>> = Vec::new();
            for host in resources
                .iter()
                .map(|each| HOST.captures(each).map(|found| found[1].to_owned()))
            {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
            match hosts.as_slice() {
                [Some(host)] => say(&format!("open {host}"), none),
                _ => say("open a web page", none),
            }
        }
        other => {
            let words = match other {
                "task" => "start a helper",
                "skill" => "use a skill",
                "websearch" => "search the web",
                "codesearch" => "search code",
                "glob" | "grep" => "look through files",
                "list" => "look at a folder",
                _ => "use a tool",
            };
            say(words, none)
        }
    }
}

/// `text` (an agent's question, a prompt) as it's shown: its Markdown and addresses in plain
/// words, cut at a word.
pub fn short_words(text: &str, longest: usize) -> String {
    cut_at_word(&plain(&plain_addresses(text)), longest)
}

/// A failure's reason, where it's in words (a sentence of a few plain words) and not a code: an
/// error name, a status number, a path, an address or something in braces. "" where it isn't.
pub fn reason_words(reason: &str) -> String {
    let first = reason
        .split('\n')
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // (Looked at as written: an address or path in it is a code, though said as a name below.)
    if first.contains(['/', '\\']) {
        return String::new();
    }
    let sentence = one_sentence(first, 60);
    if sentence.split_whitespace().count() < 3 || CODE.is_match(&sentence) {
        return String::new();
    }
    sentence
}

/// `notice` as it's shown, kept short: a title and subtitle cut at a word, a body of more than a
/// sentence as its first, in plain words.
pub fn short_notice(mut notice: Notice) -> Notice {
    fn line(text: &str) -> String {
        cut_at_word(SPACES.replace_all(text, " ").trim(), 60)
    }
    notice.title = line(&notice.title);
    notice.subtitle = notice
        .subtitle
        .as_deref()
        .map(line)
        .filter(|text| !text.is_empty());
    if let Some(body) = &notice.body
        && (body.chars().count() > SENTENCE_LONGEST || body.contains('\n'))
    {
        let sentence = one_sentence(body, SENTENCE_LONGEST);
        notice.body = Some(if sentence.is_empty() {
            cut_at_word(SPACES.replace_all(body, " ").trim(), SENTENCE_LONGEST)
        } else {
            sentence
        });
    }
    notice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_says_its_first_sentence_in_plain_words() {
        assert_eq!(
            plain("## Done\n**Fixed** the `parser`, see [docs](https://x.io)."),
            "Done Fixed the parser, see docs."
        );
        assert_eq!(
            one_sentence("Updated v1.2 of the app. Then more.", 100),
            "Updated v1.2 of the app."
        );
        assert_eq!(
            one_sentence("See e.g. the notes. Then.", 100),
            "See e.g. the notes."
        );
        assert_eq!(
            one_sentence(
                "# Title\n\nHere's what changed:\n\n- Fixed it.\n- More.",
                100
            ),
            "Here's what changed: Fixed it."
        );
        assert_eq!(one_sentence("**Done.**", 100), "Done.");
        assert_eq!(one_sentence("```\ncode\n```\nIt works", 100), "It works");
        assert_eq!(
            one_sentence(
                "Wrote /Users/me/notes/plan.md and https://www.github.com/a/b.",
                100
            ),
            "Wrote plan.md and github.com"
        );
        assert_eq!(cut_at_word("one two three four five six", 12), "one two…");
    }

    #[test]
    fn a_permission_and_a_reason_in_a_few_plain_words() {
        let lines = |each: &[&str]| {
            each.iter()
                .map(|line| (*line).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            permission_line("bash", &lines(&["cd x && FOO=1 /usr/bin/du -sh ."])),
            ("Wants to run du.".to_owned(), false)
        );
        assert_eq!(
            permission_line("bash", &lines(&["git"])),
            ("Wants to run git.".to_owned(), true)
        );
        assert_eq!(
            command_words(&lines(&["du", "git", "npm", "ls"])).0,
            "du, git and 2 more"
        );
        assert_eq!(
            permission_line("edit", &lines(&["/a/b/notes.md"])).0,
            "Wants to edit notes.md."
        );
        assert_eq!(
            permission_line("webfetch", &lines(&["https://www.example.com/x"])).0,
            "Wants to open example.com."
        );
        assert_eq!(
            reason_words("The model is overloaded right now"),
            "The model is overloaded right now"
        );
        assert_eq!(reason_words("ECONNREFUSED at port 4000"), "");
    }
}
