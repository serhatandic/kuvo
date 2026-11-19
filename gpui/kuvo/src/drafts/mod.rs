//! Design drafts rendered only under the `stand-in` feature (`shots`), never shipped in the app.

pub mod chrome;
pub mod media;
pub mod show_file;

use gpui::{Context, IntoElement, Render, Window};

pub use show_file::DraftKind;

/// Standalone GPUI view rendering one `kuvo-browser_show_file` design draft inside the stand-in
/// Kuvo window frame.
pub struct ShowFileDraftView {
    kind: DraftKind,
    media: media::SampleMedia,
}

impl ShowFileDraftView {
    pub fn new(kind: DraftKind) -> Self {
        Self { kind, media: media::SampleMedia::generate() }
    }
}

impl Render for ShowFileDraftView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let turns = show_file::conversation_turns(self.kind, &self.media, cx);
        chrome::window_shell(self.kind.badge(), turns, cx)
    }
}
