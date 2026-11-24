//! Find the commands a shell line can run for permission checks.

pub(super) struct Commands {
    parts: Vec<String>,
    pub(super) known: bool,
}

impl Commands {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            known: true,
        }
    }

    fn add(&mut self, command: String) {
        if !command.is_empty() && !self.parts.contains(&command) {
            self.parts.push(command);
        }
    }

    pub(super) fn parts(&self) -> &[String] {
        &self.parts
    }
}

pub(super) fn commands(line: &str) -> Commands {
    let mut found = Commands::new();
    read_line(line.trim(), &mut found, 0);
    found
}

pub(super) fn split(line: &str) -> Commands {
    let line = line.trim();
    let mut all = Commands::new();
    all.add(line.to_owned());
    let found = commands(line);
    for command in found.parts {
        all.add(command);
    }
    all.known = found.known;
    if !all.known {
        for broken in line.split([';', '|', '&', '\n', '`', '(', ')']) {
            all.add(broken.trim().to_owned());
        }
    }
    all
}

fn read_line(line: &str, out: &mut Commands, level: u8) {
    let joined = line.replace("\\\n", "");
    let text = joined.chars().collect::<Vec<_>>();
    scan(&mut Cursor::new(&text), None, out, level);
}

struct Cursor<'a> {
    text: &'a [char],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a [char]) -> Self {
        Self { text, at: 0 }
    }

    fn current(&self) -> Option<char> {
        self.text.get(self.at).copied()
    }

    fn ahead(&self) -> Option<char> {
        self.text.get(self.at + 1).copied()
    }

    fn starts(&self, prefix: &str) -> bool {
        let tail = &self.text[self.at..];
        tail.len() >= prefix.len()
            && tail
                .iter()
                .copied()
                .zip(prefix.chars())
                .all(|(a, b)| a == b)
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.current()?;
        self.at += 1;
        Some(ch)
    }

    fn fragment(&self, from: usize) -> String {
        self.text[from..self.at].iter().collect()
    }

    fn until_newline(&mut self) -> String {
        let from = self.at;
        while self.current().is_some_and(|ch| ch != '\n') {
            self.at += 1;
        }
        self.fragment(from)
    }
}

struct HereDocument {
    delimiter: String,
    indent: bool,
}

fn scan(input: &mut Cursor<'_>, close: Option<char>, out: &mut Commands, level: u8) {
    // Refuse to decide that arbitrarily deep evaluation is understood.
    if level >= 32 {
        out.known = false;
        input.at = input.text.len();
        return;
    }
    let mut segment = String::new();
    let mut groups = 0u32;
    let mut escaped = false;
    let mut documents = Vec::<HereDocument>::new();
    while let Some(ch) = input.current() {
        if Some(ch) == close && groups == 0 {
            add_command(&segment, out, level);
            input.next();
            return;
        }
        let start = input.at;
        let was_escaped = std::mem::take(&mut escaped);
        match ch {
            '\\' => {
                input.next();
                input.next();
                escaped = true;
            }
            '(' | '$' if arithmetic(input, &segment, out) => {}
            '\'' => {
                input.next();
                if !opaque_quote(input, '\'') {
                    out.known = false;
                }
            }
            '"' => {
                input.next();
                double_quote(input, out, level);
            }
            '`' => {
                input.next();
                scan(input, Some('`'), out, level + 1);
            }
            '$' | '<' | '>' if input.ahead() == Some('(') => {
                input.at += 2;
                scan(input, Some(')'), out, level + 1);
            }
            '#' if !was_escaped
                && (segment.is_empty() || segment.ends_with(char::is_whitespace)) =>
            {
                input.until_newline();
                continue;
            }
            '<' if input.starts("<<<") => {
                input.at += 3;
            }
            '<' if input.starts("<<") => {
                input.at += 2;
                if let Some(doc) = heredoc(input) {
                    documents.push(doc);
                }
            }
            '\n' => {
                add_command(&segment, out, level);
                segment.clear();
                input.next();
                for doc in documents.drain(..) {
                    let mut found = false;
                    while input.current().is_some() {
                        let word = input.until_newline();
                        input.next();
                        let word = if doc.indent {
                            word.trim_start_matches('\t')
                        } else {
                            &word
                        };
                        if word == doc.delimiter {
                            found = true;
                            break;
                        }
                    }
                    out.known &= found;
                }
                continue;
            }
            '&' if input.ahead() == Some('>') || segment.ends_with(['<', '>']) => {
                input.next();
            }
            '|' if segment.ends_with('>') => {
                input.next();
            }
            ';' | '|' | '&' | '(' | ')' => {
                if ch == '(' {
                    groups += 1;
                } else if ch == ')' {
                    if groups == 0 {
                        out.known = false;
                    }
                    groups = groups.saturating_sub(1);
                }
                add_command(&segment, out, level);
                segment.clear();
                input.next();
                if input.current() == Some(ch) || (ch == '|' && input.current() == Some('&')) {
                    input.next();
                }
                continue;
            }
            _ => {
                input.next();
            }
        }
        segment.push_str(&input.fragment(start));
    }
    add_command(&segment, out, level);
    if close.is_some() || groups != 0 || !documents.is_empty() {
        out.known = false;
    }
}

fn double_quote(input: &mut Cursor<'_>, out: &mut Commands, level: u8) {
    while let Some(ch) = input.current() {
        match ch {
            '"' => {
                input.next();
                return;
            }
            '\\' => {
                input.next();
                input.next();
            }
            '$' if arithmetic(input, "$", out) => {}
            '$' if input.ahead() == Some('(') => {
                input.at += 2;
                scan(input, Some(')'), out, level + 1);
            }
            '`' => {
                input.next();
                scan(input, Some('`'), out, level + 1);
            }
            _ => {
                input.next();
            }
        }
    }
    out.known = false;
}

fn opaque_quote(input: &mut Cursor<'_>, quote: char) -> bool {
    while let Some(ch) = input.next() {
        if ch == quote {
            return true;
        }
        if ch == '\\' && quote != '\'' {
            input.next();
        }
    }
    false
}

fn arithmetic(input: &mut Cursor<'_>, prefix: &str, out: &mut Commands) -> bool {
    let open = if input.starts("$((") {
        input.at + 1
    } else if input.starts("((") && prefix.trim().is_empty() {
        input.at
    } else {
        return false;
    };
    let mut depth = 0usize;
    let mut inner = None;
    for stop in open..input.text.len() {
        match input.text[stop] {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 1 && inner.is_none() {
                    inner = Some(stop);
                } else if depth == 0 {
                    if inner != stop.checked_sub(1) {
                        return false;
                    }
                    let inside = &input.text[open + 2..stop];
                    if inside.contains(&'`') || inside.windows(2).any(|pair| pair == ['$', '(']) {
                        out.known = false;
                    }
                    input.at = stop + 1;
                    return true;
                }
            }
            _ => (),
        }
    }
    false
}

fn heredoc(input: &mut Cursor<'_>) -> Option<HereDocument> {
    let start = input.at;
    let indent = input.current() == Some('-');
    if indent {
        input.next();
    }
    while input.current().is_some_and(char::is_whitespace) {
        input.next();
    }
    let quote = input.current().filter(|ch| matches!(ch, '\'' | '"'));
    if quote.is_some() {
        input.next();
    }
    let word = input.at;
    while input.current().is_some_and(|ch| {
        !ch.is_whitespace() && !matches!(ch, '\'' | '"' | '<' | '>' | ';' | '&' | '|' | '(' | ')')
    }) {
        input.next();
    }
    if input.at == word || quote.is_some_and(|want| input.current() != Some(want)) {
        input.at = start;
        return None;
    }
    let delimiter = input.fragment(word);
    if quote.is_some() {
        input.next();
    }
    Some(HereDocument { delimiter, indent })
}

fn add_command(raw: &str, out: &mut Commands, level: u8) {
    let words = shell_words(raw.trim());
    let mut original = skip_keywords(&words);
    if original.first().is_some_and(|word| word == "function") {
        original = skip_keywords(original.get(2..).unwrap_or_default());
    }
    let Some(first) = original.first() else {
        return;
    };
    if matches!(first.as_str(), "for" | "case" | "select") {
        return;
    }
    out.add(original.join(" "));
    let bare = without_prefix(original, out, level);
    if bare.is_empty() {
        return;
    }
    out.add(bare.join(" "));
    let mut plain = bare.iter().map(|word| dequote(word)).collect::<Vec<_>>();
    plain[0] = basename(&plain[0]).to_owned();
    let shown = plain
        .iter()
        .zip(bare)
        .map(|(real, written)| if plain_word(real) { real } else { written })
        .map(String::as_str)
        .collect::<Vec<_>>();
    out.add(shown.join(" "));
    if bare[0].contains(['$', '`']) || bare.iter().any(|word| ansi_quoted(word)) {
        out.known = false;
    }
    match plain[0].as_str() {
        "git" => {
            let mut at = 1;
            while let Some(word) = shown.get(at).filter(|word| word.starts_with('-')) {
                at += if matches!(
                    *word,
                    "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace"
                ) {
                    2
                } else {
                    1
                };
            }
            let rest = shown.get(at..).unwrap_or_default();
            out.add(
                std::iter::once("git")
                    .chain(rest.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        "bash" | "zsh" | "dash" | "sh" | "ksh" | "fish" | "tcsh" | "csh" | "ash" | "su" => {
            let command = plain
                .iter()
                .skip(1)
                .position(|word| {
                    word.strip_prefix('-').is_some_and(|flags| {
                        flags.contains('c') && flags.bytes().all(|flag| flag.is_ascii_lowercase())
                    })
                })
                .and_then(|at| plain.get(at + 2));
            if let Some(command) = command {
                read_line(command, out, level + 1);
            } else if plain.iter().any(|word| word == "-s")
                || !without_redirects(&plain[1..]).any(|word| !word.starts_with(['-', '+']))
            {
                out.known = false;
            }
        }
        "eval" => read_line(&plain[1..].join(" "), out, level + 1),
        "xargs" => {
            out.known = false;
            let mut at = 1;
            while let Some(word) = plain.get(at).filter(|word| word.starts_with('-')) {
                at += if word.len() == 2 && "IinPLsdEaJRS".contains(&word[1..]) {
                    2
                } else {
                    1
                };
            }
            nested_words(plain.get(at..).unwrap_or_default(), out, level);
        }
        "find" => {
            for (i, option) in plain.iter().enumerate().skip(1) {
                if !matches!(option.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
                    continue;
                }
                let command = &plain[i + 1..];
                let end = command
                    .iter()
                    .position(|word| matches!(word.as_str(), ";" | "+"));
                out.known &= end.is_some();
                nested_words(&command[..end.unwrap_or(command.len())], out, level);
            }
        }
        _ => {}
    }
}

fn nested_words(words: &[String], out: &mut Commands, level: u8) {
    if level >= 32 {
        out.known = false;
        return;
    }
    let text = words
        .iter()
        .map(|word| {
            if !word.is_empty() && plain_word(word) {
                word.to_owned()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    add_command(&text, out, level + 1);
}

fn skip_keywords(mut words: &[String]) -> &[String] {
    while words.first().is_some_and(|word| {
        matches!(
            word.as_str(),
            "if" | "then"
                | "else"
                | "elif"
                | "do"
                | "while"
                | "until"
                | "!"
                | "{"
                | "}"
                | "fi"
                | "done"
                | "esac"
        ) || word.starts_with("()")
    }) {
        words = &words[1..];
    }
    words
}

fn plain_word(word: &str) -> bool {
    word.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut letters = name.bytes();
        letters
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && letters.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

fn redirects(word: &str, heredocs: bool) -> Option<&str> {
    let tail = word.trim_start_matches(|ch: char| ch.is_ascii_digit());
    if heredocs
        && let Some(rest) = ["<<<", "<<-", "<<", "<>"]
            .into_iter()
            .find_map(|op| tail.strip_prefix(op))
    {
        return Some(rest);
    }
    ["&>>", ">>", "&>", ">&", "<&", ">", "<"]
        .into_iter()
        .find_map(|op| tail.strip_prefix(op))
}

fn without_redirects(words: &[String]) -> impl Iterator<Item = &str> {
    let mut iter = words.iter();
    std::iter::from_fn(move || {
        loop {
            let word = iter.next()?;
            match redirects(word, true) {
                None => return Some(word.as_str()),
                Some("") => {
                    iter.next();
                }
                Some(_) => {}
            }
        }
    })
}

struct Options {
    short_value: &'static str,
    long_value: &'static [&'static str],
    flags: &'static [&'static str],
}

fn wrapper(name: &str) -> Option<Options> {
    let (short_value, long_value, flags): (_, &[_], &[_]) = match name {
        "env" => (
            "uCSP",
            &["unset", "chdir", "split-string"],
            &["ignore-environment", "null", "debug"],
        ),
        "command" | "nohup" | "builtin" | "chronic" => ("", &[], &[]),
        "exec" => ("a", &[], &[]),
        "sudo" => (
            "ugCDhprtUT",
            &[
                "user",
                "group",
                "close-from",
                "chdir",
                "host",
                "prompt",
                "role",
                "type",
                "other-user",
                "command-timeout",
            ],
            &[
                "non-interactive",
                "login",
                "shell",
                "preserve-env",
                "background",
                "stdin",
                "reset-timestamp",
                "remove-timestamp",
                "set-home",
                "bell",
            ],
        ),
        "doas" => ("uC", &[], &[]),
        "time" => (
            "of",
            &["output", "format"],
            &["portability", "verbose", "append"],
        ),
        "nice" => ("n", &["adjustment"], &[]),
        "caffeinate" => ("tw", &[], &[]),
        "timeout" => (
            "sk",
            &["signal", "kill-after"],
            &["foreground", "preserve-status", "verbose"],
        ),
        "setsid" => ("", &[], &["ctty", "fork", "wait"]),
        "stdbuf" => ("ioe", &["input", "output", "error"], &[]),
        "watch" => (
            "nq",
            &["interval", "equexit"],
            &[
                "differences",
                "no-title",
                "beep",
                "errexit",
                "chgexit",
                "exec",
                "precise",
                "color",
            ],
        ),
        "ionice" => ("cnp", &["class", "classdata", "pid"], &[]),
        _ => return None,
    };
    Some(Options {
        short_value,
        long_value,
        flags,
    })
}

fn without_prefix<'a>(words: &'a [String], out: &mut Commands, level: u8) -> &'a [String] {
    let mut at = 0;
    while let Some(word) = words.get(at) {
        let dequoted = dequote(word);
        let program = basename(&dequoted);
        if assignment(word) {
            at += 1;
        } else if let Some(tail) = redirects(word, false) {
            at += if tail.is_empty() { 2 } else { 1 };
        } else if program == "coproc" {
            at += 1;
            out.known = false;
        } else if let Some(options) = wrapper(program) {
            at = skip_options(words, at + 1, program, &options, out, level);
            at += usize::from(program == "timeout");
        } else {
            break;
        }
    }
    words.get(at..).unwrap_or_default()
}

fn skip_options(
    words: &[String],
    mut at: usize,
    program: &str,
    options: &Options,
    out: &mut Commands,
    level: u8,
) -> usize {
    while let Some(word) = words
        .get(at)
        .filter(|word| word.starts_with('-') && *word != "-")
    {
        let option = dequote(word);
        if option == "--" {
            return at + 1;
        }
        let following = || {
            words
                .get(at + 1)
                .map(|word| dequote(word))
                .unwrap_or_default()
        };
        if let Some(long) = option.strip_prefix("--") {
            let (name, supplied) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            let takes_value = options.long_value.contains(&name);
            if program == "env" && name == "split-string" {
                let value = supplied.map(str::to_owned).unwrap_or_else(following);
                read_line(&value, out, level + 1);
            }
            if supplied.is_none() && !takes_value && !options.flags.contains(&name) {
                out.known = false;
            }
            at += if supplied.is_none() && takes_value {
                2
            } else {
                1
            };
        } else {
            let value = option
                .char_indices()
                .skip(1)
                .find(|(_, flag)| options.short_value.contains(*flag));
            if let Some((index, flag)) = value {
                let tail = &option[index + flag.len_utf8()..];
                if program == "env" && flag == 'S' {
                    let command = if tail.is_empty() {
                        following()
                    } else {
                        tail.to_owned()
                    };
                    read_line(&command, out, level + 1);
                }
                at += if tail.is_empty() { 2 } else { 1 };
            } else {
                at += 1;
            }
        }
    }
    at
}

fn shell_words(text: &str) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut input = Cursor::new(&chars);
    let mut words = Vec::new();
    while let Some(ch) = input.current() {
        if ch.is_whitespace() {
            input.next();
            continue;
        }
        let start = input.at;
        while let Some(ch) = input.current().filter(|ch| !ch.is_whitespace()) {
            input.next();
            match ch {
                '\\' => {
                    input.next();
                }
                '\'' | '"' | '`' => {
                    opaque_quote(&mut input, ch);
                }
                '$' if input.current() == Some('(') => {
                    let mut groups = 0;
                    while let Some(part) = input.next() {
                        match part {
                            '(' => groups += 1,
                            ')' => {
                                groups -= 1;
                                if groups == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        words.push(input.fragment(start));
    }
    words
}

fn dequote(word: &str) -> String {
    let mut out = String::new();
    let mut chars = word.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.extend(chars.next()),
            '\'' => out.extend(chars.by_ref().take_while(|ch| *ch != '\'')),
            '"' => {
                while let Some(ch) = chars.next().filter(|ch| *ch != '"') {
                    if ch == '\\'
                        && chars
                            .peek()
                            .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`'))
                    {
                        out.extend(chars.next());
                    } else {
                        out.push(ch);
                    }
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

fn ansi_quoted(word: &str) -> bool {
    let mut open = None;
    let mut chars = word.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && open != Some('\'') {
            chars.next();
        } else if open == Some(ch) {
            open = None;
        } else if open.is_none() {
            if matches!(ch, '\'' | '"') {
                open = Some(ch);
            } else if ch == '$' && chars.peek().is_some_and(|next| matches!(next, '\'' | '"')) {
                return true;
            }
        }
    }
    false
}
