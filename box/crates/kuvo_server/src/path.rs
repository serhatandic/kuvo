use percent_encoding::percent_decode_str;

fn decode(raw: &str, allow_backslashes: bool) -> Option<String> {
    for (index, byte) in raw.bytes().enumerate() {
        if byte == b'%'
            && !raw
                .as_bytes()
                .get(index + 1..index + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    let part = percent_decode_str(raw).decode_utf8().ok()?;
    if part.bytes().any(|b| b.is_ascii_control() || (!allow_backslashes && b == b'\\')) {
        return None;
    }
    Some(part.into_owned())
}

fn encode(part: &str, punctuation: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for byte in part.bytes() {
        if byte.is_ascii_alphanumeric()
            || b"-_.!~*()".contains(&byte)
            || punctuation.contains(&byte)
        {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("writing to String");
        }
    }
    result
}

fn ascii(part: &str) -> String {
    part.chars().filter(char::is_ascii).collect()
}

pub(crate) fn canonical(path: &str) -> Option<String> {
    if path.contains('#') || path.starts_with(' ') || path.ends_with(' ') {
        return None;
    }
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut result = Vec::new();
    for raw in path.split('/').filter(|raw| !raw.is_empty()) {
        let part = decode(raw, false)?;
        if part.contains('/') || matches!(ascii(&part).as_str(), "." | "..") {
            return None;
        }
        result.push(encode(&part, b"'$&+,;=:@"));
    }
    let mut path = result.join("/");
    let sessions_page = path == "kuvo/sessions/page";
    for (index, raw) in query.split('&').filter(|raw| !raw.is_empty()).enumerate() {
        path.push(if index == 0 { '?' } else { '&' });
        let mut project_key = false;
        for (index, raw) in raw.splitn(2, '=').enumerate() {
            let project_value = index == 1 && project_key;
            let part = decode(&raw.replace('+', " "), project_value)?;
            if !project_value && part.split('/').any(|piece| ascii(piece) == "..") {
                return None;
            }
            if index == 1 {
                path.push('=');
            } else {
                // The project page validates this inventory key itself.
                project_key = sessions_page && part == "project";
            }
            path.push_str(&encode(&part, b"$,;:@/?"));
        }
    }
    Some(path)
}
