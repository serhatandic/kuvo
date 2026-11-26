//! Unsupported passkeys are reported per page and remembered for site asks.
use super::{Living, page::host_of};
use serde_json::json;
use std::io;

const WORLD: &str = "kuvo-passkeys";
pub(super) const BINDING: &str = "kuvoPasskeyAsked";
pub(super) const NOTE: &str = "This site asked for a passkey, which this browser can't use. Don't choose its passkey option again: sign in another way (a password, a code or an email link), or ask the user.";

pub(super) fn event_name() -> io::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(format!("kuvo-{}", hex::encode(bytes)))
}

impl Living {
    pub(super) async fn bind_passkeys(&self, session: &str) -> io::Result<()> {
        self.pipe
            .call(
                "Runtime.addBinding",
                json!({"name":BINDING,"executionContextName":WORLD}),
                Some(session),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn install_passkeys(&self, session: &str) -> io::Result<()> {
        self.bind_passkeys(session).await?;
        let listener = format!(
            r#"window.addEventListener("{}", () => {{
  const tell = (tries) => {{
    const binding = globalThis.{BINDING};
    if (typeof binding === "function") binding("");
    else if (tries > 0) setTimeout(() => tell(tries - 1), 100);
  }};
  tell(20);
}});"#,
            self.passkey_event
        );
        self.pipe
            .call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source":listener,"worldName":WORLD,"runImmediately":true}),
                Some(session),
            )
            .await?;
        let script =
            include_str!("no_passkeys.js").replace("KUVO_PASSKEY_EVENT", &self.passkey_event);
        self.pipe
            .call(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source":script,"runImmediately":true}),
                Some(session),
            )
            .await?;
        self.pipe
            .call(
                "Target.setAutoAttach",
                json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
                Some(session),
            )
            .await?;
        Ok(())
    }

    pub(super) async fn passkey_asked(&self, target: &str) {
        let mut tabs = self.tabs.lock().await;
        let Some(host) = tabs
            .targets
            .iter()
            .find(|t| t.target_id == target)
            .and_then(|t| host_of(&t.url))
        else {
            return;
        };
        if tabs.passkeys.get(target) == Some(&host) {
            return;
        }
        self.asks.mark_passkey(&host);
        tabs.passkeys.insert(target.into(), host.clone());
        self.tell("Kuvo.passkeyAsked", json!({"targetId":target,"host":host}));
    }
}
