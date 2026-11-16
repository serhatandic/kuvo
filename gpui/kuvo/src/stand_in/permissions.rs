//! What the stand-in box says macOS allows the program it runs under (`kuvo/box/permissions`):
//! as a Mac's own box says it, each permission in words, with what's been put off kept. All made
//! up; nothing asks macOS anything.

use serde_json::{Value, json};

/// How the stand-in's permissions stand.
#[derive(Clone, Debug)]
pub struct Permissions {
    /// Whether it runs under such a program at all (a Mac's own box; the sample says not, as a
    /// box still in its app's folder does, so nothing's brought up unless a test says so).
    pub applies: bool,
    /// Whether its helper runs and has said.
    pub known: bool,
    pub screen_recording: bool,
    pub accessibility: bool,
    /// What's been put off ("Not Now").
    pub later: Vec<String>,
    /// Where its helper's bundle is.
    pub bundle: String,
    /// It can't say now (`503`).
    pub fails: bool,
}

impl Default for Permissions {
    fn default() -> Permissions {
        Permissions { applies: false, known: false, screen_recording: false, accessibility: false, later: Vec::new(), bundle: "/Users/sam/.kuvo-code/bin/Kuvo Helper.app".into(), fails: false }
    }
}

impl Permissions {
    /// A Mac's own box whose helper says neither is allowed.
    pub fn nothing_allowed() -> Permissions {
        Permissions { applies: true, known: true, ..Permissions::default() }
    }

    fn said(&self) -> Value {
        if !self.applies {
            return json!({ "applies": false, "name": null, "known": false, "screenRecording": null, "accessibility": null, "bundle": null, "list": [] });
        }
        let allowed = |allowed: bool| if self.known { json!(allowed) } else { Value::Null };
        let each = |id: &str, name: &str, line: &str, pane: &str, is: bool| {
            json!({ "id": id, "name": name, "line": line, "allowed": allowed(is), "settings": format!("x-apple.systempreferences:com.apple.preference.security?{pane}"), "byHand": true, "asks": self.known && !is && !self.later.iter().any(|later| later == id) })
        };
        json!({
            "applies": true, "name": "Kuvo Helper", "known": self.known,
            "screenRecording": allowed(self.screen_recording), "accessibility": allowed(self.accessibility),
            "bundle": self.bundle,
            "list": [
                each("screenRecording", "Screen Recording", "For agents that look at the screen.", "Privacy_ScreenCapture", self.screen_recording),
                each("accessibility", "Accessibility", "For agents that click and type in other apps.", "Privacy_Accessibility", self.accessibility),
            ],
        })
    }

    pub fn answer(&mut self, method: &str, route: &str, body: &Value) -> Option<(u16, Value)> {
        if route != "kuvo/box/permissions" {
            return None;
        }
        Some(match method {
            _ if self.fails => (503, json!({ "message": "Kuvo on this Mac isn’t answering." })),
            "GET" => (200, self.said()),
            "POST" => match body["later"].as_str() {
                Some(id @ ("screenRecording" | "accessibility")) if self.applies => {
                    if !self.later.iter().any(|later| later == id) {
                        self.later.push(id.to_owned());
                    }
                    (200, self.said())
                }
                Some("screenRecording" | "accessibility") => (409, json!({ "message": "Nothing on this computer needs macOS's permission." })),
                _ => (400, json!({ "message": "Say which permission to ask about later." })),
            },
            _ => (405, json!({})),
        })
    }
}
