//! Replying to a message, or a part of one, the same in every app (KuvoClient's `Quoting`): what
//! you quote goes into the message as Markdown ("> " lines) ahead of your words, so a reply
//! written on the phone shows its quotes on the Mac, and the other way round.

/// Something you're replying to, with your answer under it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Quote {
    pub text: String,
    pub response: String,
}

impl Quote {
    pub fn new(text: impl Into<String>) -> Quote {
        Quote { text: text.into(), response: String::new() }
    }
}

/// The line between your answers to quotes and the words after them: a Markdown rule.
pub const OWN_WORDS: &str = "---";

/// `quote` as Markdown, ahead of `text`.
pub fn with_quote(quote: &str, text: &str) -> String {
    let quote = quote.trim();
    if quote.is_empty() {
        return text.to_owned();
    }
    let block: Vec<String> = quote.split('\n').map(|line| if line.is_empty() { ">".to_owned() } else { format!("> {line}") }).collect();
    format!("{}\n\n{text}", block.join("\n"))
}

/// The message to send: each quote, as Markdown, with your answer to it under it, then the rest of
/// your words (after a line of their own when the last quote has an answer; as its answer when it
/// has none).
pub fn message(quotes: &[Quote], text: &str) -> String {
    let words = text.trim();
    let last_answered = quotes.last().is_some_and(|quote| !quote.response.trim().is_empty());
    let mut parts: Vec<String> = Vec::new();
    for quote in quotes {
        parts.push(with_quote(&quote.text, ""));
        parts.push(quote.response.trim().to_owned());
    }
    if last_answered && !words.is_empty() {
        parts.push(OWN_WORDS.to_owned());
    }
    parts.push(words.to_owned());
    parts.iter().map(|part| part.trim()).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// A message as what it quotes and what it says: each quote ("> " lines starting the message or
/// after a blank line) with the words after it, in order; the first part's quote is none when it
/// starts with words, and so is a last part of words after an answer (after `OWN_WORDS`). Lines in
/// a code block, or like ">>> x", are words.
pub fn segments(message: &str) -> Vec<(Option<String>, String)> {
    let mut segments: Vec<(Option<String>, String)> = Vec::new();
    let mut quote: Option<Vec<String>> = None;
    let mut words: Vec<String> = Vec::new();
    let close = |segments: &mut Vec<(Option<String>, String)>, quote: &mut Option<Vec<String>>, words: &mut Vec<String>| {
        let text = words.join("\n").trim().to_owned();
        let quoted = quote.take().map(|lines| lines.join("\n").trim().to_owned()).filter(|quoted| !quoted.is_empty());
        if quoted.is_some() || !text.is_empty() {
            segments.push((quoted, text));
        }
        words.clear();
    };
    let (mut in_code, mut previous_blank, mut quoting) = (false, true, false);
    for line in message.split('\n') {
        let is_quote_line = !in_code && (line == ">" || line.starts_with("> "));
        if line.starts_with("```") {
            in_code = !in_code;
        }
        if is_quote_line && (quoting || previous_blank) {
            if !quoting {
                close(&mut segments, &mut quote, &mut words);
            }
            quoting = true;
            quote.get_or_insert_with(Vec::new).push(if line == ">" { String::new() } else { line[2..].to_owned() });
        } else if !in_code && line == OWN_WORDS && previous_blank && quote.is_some() && !quoting {
            close(&mut segments, &mut quote, &mut words);
        } else {
            quoting = false;
            words.push(line.to_owned());
        }
        previous_blank = line.trim().is_empty();
    }
    close(&mut segments, &mut quote, &mut words);
    if segments.is_empty() { vec![(None, message.to_owned())] } else { segments }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_writes_its_quotes_as_markdown_and_reads_back_the_same() {
        let sent = message(&[Quote::new("Use the cache\nfor reads")], "Why not for writes?");
        assert_eq!(sent, "> Use the cache\n> for reads\n\nWhy not for writes?");
        assert_eq!(segments(&sent), [(Some("Use the cache\nfor reads".to_owned()), "Why not for writes?".to_owned())]);
        let two = message(&[Quote::new("A"), Quote::new("B")], "Both?");
        assert_eq!(two, "> A\n\n> B\n\nBoth?");
        assert_eq!(segments(&two), [(Some("A".to_owned()), String::new()), (Some("B".to_owned()), "Both?".to_owned())]);
        assert_eq!(segments("Just words"), [(None, "Just words".to_owned())]);
        assert_eq!(segments("```\n> not a quote\n```"), [(None, "```\n> not a quote\n```".to_owned())]);
    }
}
