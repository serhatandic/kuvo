//! Pictures an agent's step brought back or made (a screenshot, an image it generated, one it
//! looked at), as a step's results carry them.

use std::path::Path;

use crate::session::ToolOutput;

pub(crate) fn picture_data(data: &str, mime: &str) -> Option<ToolOutput> {
    (!data.is_empty() && mime.starts_with("image/")).then(|| ToolOutput::File {
        uri: format!("data:{mime};base64,{data}"),
        mime: mime.into(),
        name: None,
    })
}

pub(crate) fn picture_url(url: &str) -> Option<ToolOutput> {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((mime, data)) = rest.split_once(";base64,")
        && !mime.contains(',')
    {
        return picture_data(data, mime);
    }
    let path = url.strip_prefix("file://")?;
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    picture_file(&path)
}

pub(crate) fn picture_file(path: &str) -> Option<ToolOutput> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        _ => return None,
    };
    Some(ToolOutput::File {
        uri: file_url(path),
        mime: mime.into(),
        name: path.rsplit('/').next().map(str::to_owned),
    })
}

pub(crate) fn file_url(path: &str) -> String {
    const PATH: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}');
    format!(
        "file://{}",
        percent_encoding::utf8_percent_encode(path, PATH)
    )
}
