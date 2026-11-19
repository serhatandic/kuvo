//! Files an agent put in the conversation with `kuvo-browser_show_file`.

use serde::Deserialize;
use serde_json::Value;

use crate::client::{BoxClient, decode};
use crate::error::{Error, Refusal, Result};
use crate::messages::{Content, Step};
use crate::paths;
use crate::pictures::file_uri;
use crate::transport::Request;

const SHOW_FILE: &str = "kuvo-browser_show_file";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Picture,
    Pdf,
    Text,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownFile {
    pub id: String,
    pub uri: String,
    /// Absolute path on the conversation's computer.
    pub path: String,
    pub name: String,
    pub caption: Option<String>,
    pub kind: FileKind,
    /// The conversation's computer key, when supplied by the caller.
    pub computer: Option<String>,
    /// Session ID, supplied by the caller when using a session's download cache.
    pub session: Option<String>,
}

impl ShownFile {
    /// A step travels without its session or computer. Attach the context from which it was read.
    pub fn in_conversation(mut self, session: &str, computer: Option<&str>) -> Self {
        self.session = Some(session.to_owned());
        self.computer = computer.map(str::to_owned);
        self
    }
}

/// Every completed call in this step, in order. Extraction works even after a file is removed.
pub fn of_step(step: &Step) -> Vec<ShownFile> {
    step.content
        .iter()
        .enumerate()
        .filter_map(|(n, content)| match content {
            Content::Tool { name, state, .. } => of_tool(&format!("{}:{n}", step.id), name, state),
            _ => None,
        })
        .collect()
}

pub(crate) fn of_tool(step: &str, name: &str, state: &Value) -> Option<ShownFile> {
    if name != SHOW_FILE || state.get("status")?.as_str()? != "completed" {
        return None;
    }
    let path = state.pointer("/input/path")?.as_str()?;
    let name = path.rsplit('/').next().filter(|name| !name.is_empty())?;
    let caption = state
        .pointer("/input/caption")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    Some(ShownFile {
        id: format!("{step}-shown"),
        uri: file_uri(path),
        path: path.to_owned(),
        name: name.to_owned(),
        caption: caption.map(str::to_owned),
        kind: guess_type(path).0,
        computer: None,
        session: None,
    })
}

/// The kind and MIME by file extension, usable before the box has answered.
pub fn guess_type(path: &str) -> (FileKind, &'static str) {
    use FileKind::*;

    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => (Picture, "image/png"),
        Some("jpg" | "jpeg") => (Picture, "image/jpeg"),
        Some("gif") => (Picture, "image/gif"),
        Some("webp") => (Picture, "image/webp"),
        Some("heic") => (Picture, "image/heic"),
        Some("heif") => (Picture, "image/heif"),
        Some("bmp") => (Picture, "image/bmp"),
        Some("tif" | "tiff") => (Picture, "image/tiff"),
        Some("ico") => (Picture, "image/vnd.microsoft.icon"),
        Some("svg") => (Picture, "image/svg+xml"),
        Some("pdf") => (Pdf, "application/pdf"),
        Some("md") => (Text, "text/markdown"),
        Some("csv") => (Text, "text/csv"),
        Some("json") => (Text, "application/json"),
        Some("html") => (Text, "text/html"),
        Some("css") => (Text, "text/css"),
        Some("js") => (Text, "text/javascript"),
        Some("xml") => (Text, "application/xml"),
        Some("yaml" | "yml") => (Text, "text/yaml"),
        Some(
            "txt" | "ts" | "tsx" | "jsx" | "rs" | "py" | "sh" | "zsh" | "bash" | "log" | "toml",
        ) => (Text, "text/plain"),
        Some("mp4") => (Other, "video/mp4"),
        Some("mov") => (Other, "video/quicktime"),
        Some("mp3") => (Other, "audio/mpeg"),
        Some("wav") => (Other, "audio/wav"),
        _ => (Other, "application/octet-stream"),
    }
}

/// Metadata for a file the box can open. Missing files are returned as `Error::NotFound`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub size: u64,
    pub kind: FileKind,
    pub mime: String,
}

#[derive(Deserialize)]
struct InfoReply {
    exists: bool,
    #[serde(flatten)]
    info: Option<FileInfo>,
}

/// Maximum number of bytes a box will return from `kuvo/file`.
pub const FILE_MOST: usize = 25 * 1024 * 1024;

impl BoxClient {
    /// A file on this box's computer, by its `file://` address: its bytes, at most `max`, and MIME
    /// when the transport provides it.
    pub fn file_bytes(&self, uri: &str, max: usize) -> Result<(Vec<u8>, Option<String>)> {
        self.download_file(uri, max, "")
    }

    /// Downloads the bytes and whether the source had execute bits at the time it was opened.
    /// An older box may send the bytes without the source mode.
    pub fn file_for_open(&self, uri: &str) -> Result<(Vec<u8>, Option<bool>)> {
        let (bytes, content_type) = self.download_file(uri, FILE_MOST, "&withMode=1")?;
        let executable = match content_type.as_deref() {
            Some("application/vnd.kuvo.file; executable=0") => Some(false),
            Some("application/vnd.kuvo.file; executable=1") => Some(true),
            _ => None,
        };
        Ok((bytes, executable))
    }

    fn download_file(
        &self,
        uri: &str,
        max: usize,
        extra: &str,
    ) -> Result<(Vec<u8>, Option<String>)> {
        let response = self.send(&Request::get(format!(
            "kuvo/file?uri={}&max={max}{extra}",
            paths::query_value(uri)
        )))?;
        if !response.is_success() {
            let refusal = Refusal::from_answer(response.status, &response.body);
            // A box that doesn't have the route also answers 404, with an empty object.
            if refusal.status == 404
                && (refusal.reason.as_deref() == Some("fileNotFound")
                    || refusal.message == "That file isn’t there anymore.")
            {
                return Err(Error::NotFound);
            }
            return Err(Error::Refused(refusal));
        }
        Ok((response.body, response.content_type))
    }

    /// A file's metadata on this box's computer, without reading its contents.
    pub fn file_info(&self, uri: &str) -> Result<FileInfo> {
        let response = self.send(&Request::get(format!(
            "kuvo/file/info?uri={}",
            paths::query_value(uri)
        )))?;
        let reply: InfoReply = decode(&response)?;
        if !reply.exists {
            return Err(Error::NotFound);
        }
        reply
            .info
            .ok_or_else(|| Error::BadAnswer("The box did not send file metadata.".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Closer, Response, Stream, Transport};
    use serde_json::json;

    #[test]
    fn extracts_each_completed_show_file_in_step_order_without_checking_disk() {
        let show = |state: Value| json!({ "type": "tool", "name": SHOW_FILE, "state": state });
        let step: Step = serde_json::from_value(json!({
            "id": "msg_1",
            "time": {},
            "content": [
                show(json!({ "status": "completed", "input": { "path": "/somewhere/chart #1.PNG", "caption": "The chart" } })),
                { "type": "text", "text": "Both files" },
                show(json!({ "status": "completed", "input": { "path": "/no-longer-there/report.pdf" } })),
                show(json!({ "status": "completed", "input": { "caption": "No path given" } })),
                show(json!({ "status": "failed", "input": { "path": "/not-shown.txt" } })),
                show(json!({ "status": "completed", "input": { "path": "/report.txt", "caption": "" } })),
                show(json!({ "status": "completed", "input": { "path": "/archive.zip" } })),
            ]
        })).unwrap();
        let files = of_step(&step);
        assert_eq!(
            files.iter().map(|file| file.kind).collect::<Vec<_>>(),
            [
                FileKind::Picture,
                FileKind::Pdf,
                FileKind::Text,
                FileKind::Other
            ]
        );
        assert_eq!(files[0].id, "msg_1:0-shown");
        assert_eq!(files[0].name, "chart #1.PNG");
        assert_eq!(files[0].uri, "file:///somewhere/chart%20%231.PNG");
        assert_eq!(files[0].caption.as_deref(), Some("The chart"));
        assert_eq!(files[1].path, "/no-longer-there/report.pdf");
        assert_eq!(files[1].caption, None);
        assert_eq!(files[2].caption, None);
        let remote = files[0].clone().in_conversation("ses_1", Some("devbox"));
        assert_eq!(remote.computer.as_deref(), Some("devbox"));
        assert_eq!(remote.session.as_deref(), Some("ses_1"));
    }

    struct BoxAnswer(Result<Response>);

    impl Transport for BoxAnswer {
        fn send(&self, request: &Request) -> Result<Response> {
            if request.path == "kuvo/info" {
                return Ok(response(
                    200,
                    json!({ "kind": "machine", "name": "test", "features": crate::gate::REQUIRED }),
                ));
            }
            assert_eq!(request.method, "GET");
            assert!(request.path.starts_with("kuvo/file"));
            assert!(
                request
                    .path
                    .contains(&paths::query_value("file:///report.pdf"))
            );
            if self.0.as_ref().is_ok_and(|response| {
                response
                    .content_type
                    .as_deref()
                    .is_some_and(|mime| mime.starts_with("application/vnd.kuvo.file;"))
            }) {
                assert!(request.path.ends_with("&withMode=1"));
            }
            self.0.clone()
        }
        fn open(&self, _: &Request, _: &dyn Fn(Closer) -> bool) -> Result<Stream> {
            unreachable!()
        }
        fn streams(&self) -> bool {
            false
        }
        fn describe(&self) -> String {
            "test box".into()
        }
    }

    fn response(status: u16, body: Value) -> Response {
        Response {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            content_type: Some("application/json".into()),
        }
    }

    #[test]
    fn downloads_read_the_source_mode_and_distinguish_an_older_box() {
        const URI: &str = "file:///report.pdf";
        for (content_type, mode) in [
            ("application/vnd.kuvo.file; executable=1", Some(true)),
            ("application/vnd.kuvo.file; executable=0", Some(false)),
            ("application/pdf", None),
        ] {
            let bytes = b"extensionless without a shebang".to_vec();
            let client = BoxClient::new(BoxAnswer(Ok(Response {
                status: 200,
                content_type: Some(content_type.into()),
                body: bytes.clone(),
            })));
            assert_eq!(client.file_for_open(URI).unwrap(), (bytes, mode));
        }
    }

    #[test]
    fn a_missing_file_an_older_route_and_a_lost_connection_are_distinct() {
        const URI: &str = "file:///report.pdf";
        let missing = BoxClient::new(BoxAnswer(Ok(response(
            404,
            json!({ "message": "That file isn’t there anymore." }),
        ))));
        assert_eq!(missing.file_bytes(URI, FILE_MOST), Err(Error::NotFound));
        let missing = BoxClient::new(BoxAnswer(Ok(response(
            404,
            json!({ "message": "File unavailable", "reason": "fileNotFound" }),
        ))));
        assert_eq!(missing.file_bytes(URI, FILE_MOST), Err(Error::NotFound));
        let missing = BoxClient::new(BoxAnswer(Ok(response(
            200,
            json!({ "exists": false, "name": "report.pdf", "size": null, "kind": null, "mime": null }),
        ))));
        assert_eq!(missing.file_info(URI), Err(Error::NotFound));

        let old = BoxClient::new(BoxAnswer(Ok(response(404, json!({})))));
        assert!(matches!(
            old.file_info(URI),
            Err(Error::Refused(Refusal {
                older_box: true,
                ..
            }))
        ));
        assert!(matches!(
            old.file_bytes(URI, FILE_MOST),
            Err(Error::Refused(Refusal {
                older_box: true,
                ..
            }))
        ));
        let unreachable = BoxClient::new(BoxAnswer(Err(Error::Unreachable(
            "The connection dropped.".into(),
        ))));
        assert!(matches!(
            unreachable.file_info(URI),
            Err(Error::Unreachable(_))
        ));
        assert!(matches!(
            unreachable.file_bytes(URI, FILE_MOST),
            Err(Error::Unreachable(_))
        ));

        let present = BoxClient::new(BoxAnswer(Ok(response(
            200,
            json!({
                "exists": true, "name": "report.pdf", "size": 42, "kind": "pdf", "mime": "application/pdf"
            }),
        ))));
        assert_eq!(
            present.file_info(URI).unwrap(),
            FileInfo {
                name: "report.pdf".into(),
                size: 42,
                kind: FileKind::Pdf,
                mime: "application/pdf".into(),
            }
        );
    }
}
