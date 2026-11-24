//! What a conversation was last told of Kuvo Code's instructions. Claude Code takes them only as
//! it starts, and a conversation it goes on with keeps the ones it started with: so when they
//! changed (it moved to another computer, its worktree or project's folders changed), the next
//! message says what changed, once, ahead of yours.

/// What to say ahead of a message, when the instructions are `now` and the conversation was told
/// `before` (none: not known, so they're all said). Nothing when nothing changed.
pub(super) fn instructions_note(before: Option<&str>, now: &str) -> Option<String> {
    if before == Some(now) {
        return None;
    }
    let Some(before) = before else {
        if now.trim().is_empty() {
            return None;
        }
        return Some(format!(
            "<system-reminder>\nKuvo Code's instructions for you, as they are now. Where they differ from what you were told before, these hold:\n\n{}\n</system-reminder>",
            now.trim()
        ));
    };
    let was = paragraphs(before);
    let is = paragraphs(now);
    let added = is
        .iter()
        .filter(|part| !was.contains(part))
        .copied()
        .collect::<Vec<_>>();
    let removed = was
        .iter()
        .filter(|part| !is.contains(part))
        .copied()
        .collect::<Vec<_>>();
    // (Only their spacing or order changed.)
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    let mut parts =
        vec!["Kuvo Code changed your instructions since you were last told them.".to_owned()];
    if !added.is_empty() {
        parts.push(format!(
            "Where they differ from what you were told before, this holds now:\n\n{}",
            added.join("\n\n")
        ));
    }
    if !removed.is_empty() {
        parts.push(format!(
            "Replaced or taken out:\n\n{}",
            removed.join("\n\n")
        ));
    }
    Some(format!(
        "<system-reminder>\n{}\n</system-reminder>",
        parts.join("\n\n")
    ))
}

/// Paragraphs, as the instructions' entries are joined (an entry may hold several).
fn paragraphs(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("\n\n") {
        parts.push(&rest[..at]);
        rest = rest[at..].trim_start_matches('\n');
    }
    parts.push(rest);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_changed_once() {
        let old = "You work in the worktree at /old.\n\nYou moved here.";
        let new = "You work in the worktree at /new.\n\nYou moved here.";
        let note = instructions_note(Some(old), new).unwrap();
        assert!(note.contains("this holds now:\n\nYou work in the worktree at /new."));
        assert!(note.contains("Replaced or taken out:\n\nYou work in the worktree at /old."));
        assert_eq!(instructions_note(Some(new), new), None);
        assert_eq!(instructions_note(Some("a\n\nb"), "b\n\n\na"), None);
        assert!(instructions_note(None, "a")
            .unwrap()
            .contains("as they are now"));
        assert_eq!(instructions_note(None, " "), None);
    }
}
