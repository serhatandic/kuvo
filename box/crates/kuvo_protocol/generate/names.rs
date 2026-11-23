use std::collections::HashSet;

#[derive(Default)]
pub struct Names(HashSet<String>);

impl Names {
    pub fn take(&mut self, base: &str) -> String {
        let mut name = base.to_owned();
        let mut suffix = 2;
        while !self.0.insert(name.clone()) {
            name = format!("{base}{suffix}");
            suffix += 1;
        }
        name
    }
}

fn words(value: &str) -> impl Iterator<Item = String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            let bytes = word.as_bytes();
            let starts = (1..bytes.len()).filter(|&index| {
                bytes[index].is_ascii_uppercase()
                    && (bytes[index - 1].is_ascii_lowercase()
                        || bytes.get(index + 1).is_some_and(u8::is_ascii_lowercase))
            });
            let mut start = 0;
            starts.chain(std::iter::once(bytes.len())).map(move |end| {
                let part = word[start..end].to_ascii_lowercase();
                start = end;
                part
            })
        })
}

pub fn pascal(value: &str) -> String {
    let name: String = words(value)
        .map(|mut word| {
            word[..1].make_ascii_uppercase();
            word
        })
        .collect();
    match name.as_bytes().first() {
        Some(first) if first.is_ascii_alphabetic() && name != "Self" => name,
        _ => format!("Value{name}"),
    }
}

pub fn snake(value: &str) -> String {
    let name = words(value).collect::<Vec<_>>().join("_");
    match name.as_str() {
        "as" | "async" | "await" | "break" | "const" | "continue" | "crate" | "dyn" | "else"
        | "enum" | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop"
        | "match" | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "self" | "static"
        | "struct" | "super" | "trait" | "true" | "type" | "unsafe" | "use" | "where" | "while"
        | "abstract" | "become" | "box" | "do" | "final" | "gen" | "macro" | "override"
        | "priv" | "try" | "typeof" | "unsized" | "virtual" | "yield" => format!("field_{name}"),
        _ if name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) => name,
        _ => format!("field_{name}"),
    }
}
