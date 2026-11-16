//! A message's words naming the pictures it carries (KuvoClient's `ImageReferences`): "[Image 1]"
//! for its first picture, "[Image 2]" for its second, in the order they're attached. A picture
//! added to a composer puts its name where you're writing, so you can say which one you mean, and
//! the agent reads the same words beside the pictures. Removing a picture takes its name out and
//! the later ones down a number.

/// The name of the `number`th picture, as written.
pub fn name(number: usize) -> String {
    format!("[Image {number}]")
}

/// Each picture named in `text`: where, and its number.
pub fn find(text: &str) -> Vec<(std::ops::Range<usize>, usize)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("[Image ").map(|at| at + from) {
        let digits_from = at + "[Image ".len();
        let digits: String = text[digits_from..].chars().take_while(char::is_ascii_digit).collect();
        let end = digits_from + digits.len();
        if (1..=3).contains(&digits.len()) && !digits.starts_with('0') && text[end..].starts_with(']') {
            found.push((at..end + 1, digits.parse().unwrap_or(0)));
            from = end + 1;
        } else {
            from = digits_from;
        }
    }
    found
}

/// `text` without the `number`th picture's names (and a space each leaves behind), the later ones
/// a number down, as they are once it's gone.
pub fn removing(number: usize, text: &str) -> String {
    let mut text = text.to_owned();
    for (range, found) in find(&text).into_iter().rev() {
        if found == number {
            let mut range = range;
            // The space after it goes with it; else, the one before it (at the message's end).
            if text[range.end..].starts_with(' ') {
                range.end += 1;
            } else if range.start > 0 && text[..range.start].ends_with(' ') {
                range.start -= 1;
            }
            text.replace_range(range, "");
        } else if found > number {
            text.replace_range(range, &name(found - 1));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pictures_name_goes_with_it_and_the_later_ones_move_down() {
        let text = "Compare [Image 1] with [Image 2] and [Image 3]";
        assert_eq!(find(text).iter().map(|(_, number)| *number).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(removing(2, text), "Compare [Image 1] with and [Image 2]");
        assert_eq!(removing(3, text), "Compare [Image 1] with [Image 2] and");
        assert!(find("[Image 0] [Image x] [Image 12").is_empty());
    }
}
