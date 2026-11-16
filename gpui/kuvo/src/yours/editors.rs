//! General's "Open code and text in": the editors people use for code that are on this Mac, and
//! opening a file of this computer in the one picked (else in the Mac's default text editor). Only a
//! file on this computer opens: Files leaves the button out for another computer's.

use std::path::Path;

use gpui::{App, AppContext as _};

/// Editors people use for code, by bundle identifier, as General offers them when they're here
/// (as 0.7 did).
pub const KNOWN: &[&str] = &[
    "com.microsoft.VSCode",
    "com.todesktop.230313mzl4w4u92",
    "com.exafunction.windsurf",
    "dev.zed.Zed",
    "com.apple.dt.Xcode",
    "com.sublimetext.4",
    "com.panic.Nova",
    "com.barebones.bbedit",
    "com.jetbrains.intellij",
    "com.jetbrains.WebStorm",
    "com.jetbrains.pycharm",
    "com.vscodium",
    "com.apple.TextEdit",
];

/// An editor on this Mac: its bundle identifier and its name, as its app is named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub id: String,
    pub name: String,
}

/// The known editors on this Mac, in [`KNOWN`]'s order.
pub fn installed() -> Vec<Editor> {
    KNOWN.iter().filter_map(|id| find(id)).collect()
}

/// The editor with this bundle identifier, where it's on this Mac.
pub fn find(id: &str) -> Option<Editor> {
    #[cfg(target_os = "macos")]
    {
        let app = super::mac::app_with_id(id)?;
        Some(Editor { id: id.to_owned(), name: app.file_stem()?.to_string_lossy().into_owned() })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        None
    }
}

/// The editor picked in General, where it's still on this Mac.
pub fn picked(cx: &App) -> Option<Editor> {
    find(&super::now(cx).general.editor?)
}

/// Opens a file of this computer to read and edit: in `editor` (by its bundle identifier) where
/// there's one, else in the Mac's default text editor. Never in the app a file's type names, which
/// for a file an agent wrote could run it (a `.command` runs in Terminal).
pub fn open(path: &Path, editor: Option<&str>, cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let (editor, path) = (editor.map(str::to_owned), path.to_owned());
        // (`open` hands it to Launch Services and returns: waited for off the UI’s thread. The path
        // is absolute, so never read as an option.)
        cx.background_spawn(async move {
            let mut open = std::process::Command::new("/usr/bin/open");
            match editor {
                Some(id) => open.arg("-b").arg(id),
                None => open.arg("-t"),
            };
            let _ = open.arg(path).status();
        })
        .detach();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, editor, cx);
    }
}
