//! Recover the blocks kept in older messages.

use http::StatusCode;
use kuvo_protocol::routes::post_kuvo_messages_parts as read;
use kuvo_server::{Access, Refusal, Registry, RouteReply};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const WIDGET: Block = Block {
    opening: "<from-widget>",
    closing: "</from-widget>",
    headings: &[
        "The user sent this from a widget shown in this conversation (show_widget), titled:",
    ],
};
const ATTACHED: Block = Block {
    opening: "<attached-files>",
    closing: "</attached-files>",
    headings: &[
        "The attached files are on disk here (what's in them is in this message already):",
        "Open these attached files with your tools if you need them:",
        "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):",
    ],
};
const REVIEW: Block = Block {
    opening: "<review-comments>",
    closing: "</review-comments>",
    headings: &["Comments on lines of the current changes:"],
};
const NAMED: Block = Block {
    opening: "<named-agents>",
    closing: "</named-agents>",
    headings: &[
        "Agents the user named with & in this message, by their address. Read one's conversation with read_conversation only when what the user asks needs what it knows or did, and write to it with send_message only to ask it something or hand it work; when the user only mentions one, do neither:",
        "Agents the user named with & in this message. Read one's conversation with read_conversation, or write to it with send_message, by its address:",
    ],
};
const PHONE: Block = Block {
    opening: "<from-phone>",
    closing: "</from-phone>",
    headings: &["The user sent this from Kuvo Code on their iPhone."],
};
const UNSENT: &str = " (on the user's Mac: it couldn't be copied to this machine)";

struct Block {
    opening: &'static str,
    closing: &'static str,
    headings: &'static [&'static str],
}

impl Block {
    fn escape(&self, text: &mut String) {
        while let Some((start, _)) = self.find(text) {
            text.insert(start + self.opening.len(), ' ');
        }
    }

    fn append(&self, text: &mut String, lines: &[&str]) {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(self.opening);
        for line in lines {
            text.push('\n');
            text.push_str(line);
        }
        text.push('\n');
        text.push_str(self.closing);
    }

    fn take<'a>(&self, text: &'a str) -> Option<(&'a str, Vec<&'a str>)> {
        self.find(text)
            .map(|(start, lines)| (trim(&text[..start]), lines))
    }

    fn find<'a>(&self, text: &'a str) -> Option<(usize, Vec<&'a str>)> {
        let body = text
            .trim_end_matches(is_js_space)
            .strip_suffix(self.closing)?
            .strip_suffix('\n')?;
        let opening = format!("{}\n", self.opening);
        body.rmatch_indices(&opening).find_map(|(offset, _)| {
            let before = &body[..offset];
            if offset > 0 && !before.ends_with('\n') {
                return None;
            }
            let lines: Vec<_> = body[offset + opening.len()..].split('\n').collect();
            self.headings
                .contains(lines.first()?)
                .then_some((offset, lines))
        })
    }
}

#[derive(Serialize, Default)]
pub(crate) struct Parts<'a> {
    pub words: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    widget: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    comments: Vec<Comment<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    paths: Vec<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    agents: Vec<Agent<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mail: Option<Agent<'a>>,
}

#[derive(Serialize)]
pub(crate) struct Agent<'a> {
    pub name: &'a str,
    session: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Side {
    New,
    Old,
}

#[derive(Serialize)]
struct Comment<'a> {
    file: &'a str,
    line: i64,
    side: Side,
    snippet: &'a str,
    text: String,
}

fn trim(text: &str) -> &str {
    // JavaScript trim includes BOM and excludes NEL.
    text.trim_matches(is_js_space)
}

fn is_js_space(c: char) -> bool {
    c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}')
}

pub(crate) fn read_parts(text: &str) -> Parts<'_> {
    if let Some((words, mail)) = mail(text) {
        return Parts {
            words,
            mail: Some(mail),
            ..Parts::default()
        };
    }
    let (text, phone) = match PHONE.take(text) {
        Some((text, _)) => (text, Some(true)),
        None => (text, None),
    };
    let (text, paths) = match ATTACHED.take(text) {
        Some((before, lines)) => {
            let mut heading = "";
            let mut paths = Vec::new();
            for line in lines {
                if ATTACHED.headings.contains(&line) {
                    heading = line;
                } else if line.starts_with('/') && heading != ATTACHED.headings[0] {
                    paths.push(line.strip_suffix(UNSENT).unwrap_or(line));
                }
            }
            (before, paths)
        }
        None => (text, Vec::new()),
    };
    let (text, comments) = match REVIEW.take(text) {
        Some((before, lines)) => (before, review_comments(&lines)),
        None => (text, Vec::new()),
    };
    let (text, agents) = match NAMED.take(text) {
        Some((before, lines)) => {
            let agents = lines
                .iter()
                .skip(1)
                .filter_map(|line| {
                    let line = line.strip_prefix("- &")?;
                    let open = line.find(" (ses_")?;
                    let name = &line[..open];
                    let (session, _) = line[open + 2..].split_once(')')?;
                    (!name.is_empty()).then_some(Agent { name, session })
                })
                .collect();
            (before, agents)
        }
        None => (text, Vec::new()),
    };
    let (words, widget) = match WIDGET.take(text) {
        Some((before, lines)) => (before, Some(trim(&lines[1..].join(" ")).to_owned())),
        None => (text, None),
    };
    Parts {
        words,
        widget,
        comments,
        paths,
        agents,
        phone,
        mail: None,
    }
}

fn mail(text: &str) -> Option<(&str, Agent<'_>)> {
    const OPENING: &str = "[Message from ";
    let (first, rest) = text.split_at(text.find('\n').unwrap_or(text.len()));
    let (name, address) = first.rsplit_once(" · ")?;
    let name = name.strip_prefix(OPENING)?;
    let (session, _) = address.split_once(']')?;
    if !(session.starts_with("ses_") || session == "unknown") || session.contains(' ') {
        return None;
    }
    let words = rest
        .split_once("\n\n")
        .map_or_else(|| rest.trim_start_matches('\n'), |(_, words)| words);
    Some((words, Agent { name, session }))
}

fn review_comments<'a>(lines: &[&'a str]) -> Vec<Comment<'a>> {
    let mut comments: Vec<Comment<'a>> = Vec::new();
    for (index, &line) in lines.iter().enumerate().skip(1) {
        if lines[index - 1].is_empty()
            && lines
                .get(index + 1)
                .is_some_and(|line| line.starts_with("> "))
            && let Some(header) = comment_header(line)
        {
            comments.push(header);
        } else if let Some(comment) = comments.last_mut() {
            if comment.snippet.is_empty()
                && comment.text.is_empty()
                && let Some(snippet) = line.strip_prefix("> ")
            {
                comment.snippet = snippet;
            } else {
                if !comment.text.is_empty() {
                    comment.text.push('\n');
                }
                comment.text.push_str(line);
            }
        }
    }
    for comment in &mut comments {
        comment.text = comment
            .text
            .trim_matches(|c: char| c == '\u{feff}' || c.is_whitespace())
            .to_owned();
    }
    comments
}

fn comment_header(text: &str) -> Option<Comment<'_>> {
    let header = text.strip_prefix('[')?.strip_suffix(']')?;
    let (header, side) = match header.strip_suffix(" (removed)") {
        Some(header) => (header, Side::Old),
        None => (header, Side::New),
    };
    let (file, line) = header.rsplit_once(':')?;
    let digits = line.strip_prefix(['+', '-']).unwrap_or(line);
    if digits.len() > 18 {
        return None;
    }
    Some(Comment {
        file,
        line: line.parse().ok()?,
        side,
        snippet: "",
        text: String::new(),
    })
}

#[derive(Deserialize)]
struct Texts {
    texts: Vec<String>,
}

pub(crate) fn register(routes: &mut Registry) {
    let invalid = || Refusal::new(StatusCode::BAD_REQUEST, "Send the messages' texts (texts).");
    routes.register::<read::Spec, _, _>(Access::Any, move |call| async move {
        let given: Texts = serde_json::from_slice(&call.request.body).map_err(|_| invalid())?;
        if given.texts.len() > 500 {
            return Err(Refusal::new(
                StatusCode::BAD_REQUEST,
                "Send at most 500 texts at once.",
            ));
        }
        let parts = given
            .texts
            .iter()
            .map(|text| read_parts(text))
            .collect::<Vec<_>>();
        let body: read::Status200 =
            serde_json::from_value(json!({ "parts": parts })).map_err(|_| {
                Refusal::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The message parts could not be read.",
                )
            })?;
        Ok(RouteReply::json::<200>(body))
    });
}

const PHONE_NOTE: &str = "They read your reply on a small screen, and can't open file paths, links to files on the computer or Kuvo Code's side panel there. Keep your reply short and easy to read on a phone. To show them a file (a picture, a chart, a PDF, a document you made or found), use show_file: it puts the file in this conversation, where they see it. A widget (show_widget) shows there too: keep it narrow, and make it work with taps rather than hovering.";

pub(crate) struct Message {
    pub text: String,
    pub parts: Option<Value>,
    pub words: Option<String>,
}

pub(crate) fn compose(
    given: &Map<String, Value>,
    has_files: bool,
    phone: bool,
    legacy_queue: bool,
) -> Result<Message, &'static str> {
    if let Some(text) = given.get("text").and_then(Value::as_str) {
        if text.trim().is_empty() && !has_files {
            return Err(if legacy_queue {
                "Send what was written (words)."
            } else {
                "There's nothing to send."
            });
        }
        let kept = given.get("kept") == Some(&Value::Bool(true)) && legacy_queue;
        let again = !legacy_queue || given.get("again") == Some(&Value::Bool(true));
        let mut text = text.to_owned();
        if !kept && !again && phone && !text.trim_end().ends_with(PHONE.closing) {
            PHONE.append(&mut text, &[PHONE.headings[0], PHONE_NOTE]);
        }
        let parts = read_parts(&text);
        let parsed = (again || parts.words != text || parts.mail.is_some()).then(|| json!(parts));
        let provided = given
            .get("metadata")
            .and_then(|metadata| metadata.pointer("/kuvo/parts"))
            .filter(|parts| parts["words"].is_string());
        return Ok(Message {
            parts: if kept {
                provided.cloned().or(parsed)
            } else {
                parsed
            },
            words: given
                .get("words")
                .and_then(Value::as_str)
                .map(str::to_owned),
            text,
        });
    }
    let words = given
        .get("words")
        .and_then(Value::as_str)
        .ok_or("Send what was written (words).")?;
    let mut text = trim(words).to_owned();
    let widget = given.get("widget").and_then(Value::as_str).map(|widget| {
        widget
            .split(['\n', '\r', '\x0b', '\x0c', '\u{85}', '\u{2028}', '\u{2029}'])
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .replace('&', "＆")
    });
    let comments = given
        .get("comments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(given_comment)
        .take(500)
        .collect::<Vec<_>>();
    let lines = |key| {
        given
            .get("notes")
            .and_then(|notes| notes.get(key))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(1000)
            .collect::<Vec<_>>()
    };
    let (included, paths, unsent) = (lines("included"), lines("paths"), lines("unsent"));
    if text.is_empty()
        && widget.is_none()
        && comments.is_empty()
        && !has_files
        && included.is_empty()
        && paths.is_empty()
        && unsent.is_empty()
    {
        return Err("There's nothing to send.");
    }

    let mut shown = None;
    if let Some(widget) = &widget {
        shown = Some(text.clone());
        text = trim(&format!(
            "{words}\n\n{}\n{}\n{widget}\n{}",
            WIDGET.opening, WIDGET.headings[0], WIDGET.closing
        ))
        .to_owned();
    }
    NAMED.escape(&mut text);
    REVIEW.escape(&mut text);
    if !comments.is_empty() {
        shown.get_or_insert_with(|| trim(&text).to_owned());
        let mut lines = vec![REVIEW.headings[0].to_owned()];
        for comment in &comments {
            let removed = if matches!(comment.side, Side::Old) {
                " (removed)"
            } else {
                ""
            };
            lines.extend([
                String::new(),
                format!("[{}:{}{removed}]", comment.file, comment.line),
                format!("> {}", comment.snippet),
                comment.text.clone(),
            ]);
        }
        REVIEW.append(
            &mut text,
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        );
    }
    ATTACHED.escape(&mut text);
    let mut notes = Vec::new();
    for (heading, contents) in
        ATTACHED
            .headings
            .iter()
            .zip([included.as_slice(), paths.as_slice(), unsent.as_slice()])
    {
        if !contents.is_empty() {
            notes.push(*heading);
            notes.extend(contents);
        }
    }
    if !notes.is_empty() {
        shown.get_or_insert_with(|| trim(&text).to_owned());
        ATTACHED.append(&mut text, &notes);
    }
    if phone && !text.trim_end().ends_with(PHONE.closing) {
        shown.get_or_insert_with(|| trim(&text).to_owned());
        let empty = text.is_empty();
        PHONE.append(&mut text, &[PHONE.headings[0], PHONE_NOTE]);
        if empty {
            text.insert_str(0, "\n\n");
        }
    }
    let parts = json!(Parts {
        words: shown.as_deref().unwrap_or(&text),
        widget,
        comments: comments
            .into_iter()
            .map(|mut comment| {
                comment.text = trim(&comment.text).to_owned();
                comment
            })
            .collect(),
        paths: paths.into_iter().chain(unsent).collect(),
        phone: phone.then_some(true),
        ..Parts::default()
    });
    Ok(Message {
        text,
        parts: Some(parts),
        words: Some(trim(words).to_owned()),
    })
}

fn given_comment(value: &Value) -> Option<Comment<'_>> {
    let line = value["line"].as_i64()?;
    if line.unsigned_abs() > (1 << 53) - 1 {
        return None;
    }
    Some(Comment {
        file: value["file"].as_str()?,
        line,
        side: if value["side"] == "old" {
            Side::Old
        } else {
            Side::New
        },
        snippet: value["snippet"].as_str()?,
        text: value["text"].as_str()?.to_owned(),
    })
}
