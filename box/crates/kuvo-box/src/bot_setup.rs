//! What bots read of this computer's setup: your personalities, your time zone and its link's id.

use kuvo_setup::SetupHandle;
use serde_json::Value;

pub(crate) struct BotSetup(pub SetupHandle);

impl kuvo_work::bots::setup::Setup for BotSetup {
    fn personalities(&self) -> Vec<Value> {
        self.0.personalities()
    }

    fn time_zone(&self) -> Option<(String, Option<Vec<u8>>)> {
        self.0.time_zone()
    }

    fn link_id(&self) -> Option<String> {
        self.0.link_id()
    }
}
