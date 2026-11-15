//! Your setup as a box says it (PROTOCOL.md, "Your setup", `kuvo/setup`): here only whether you've
//! had Kuvo Code's first run with this box (`welcomed`, `setup.welcomed`), kept on the box so
//! every client of it agrees, and saying it's done (or to be had again).

use serde::Deserialize;
use serde_json::json;

use crate::client::BoxClient;
use crate::error::Result;
use crate::transport::Request;

#[derive(Deserialize)]
struct Welcomed {
    welcomed: bool,
}

impl BoxClient {
    /// Whether you've had the first run with this box.
    pub fn welcomed(&self) -> Result<bool> {
        self.get::<Welcomed>("kuvo/setup").map(|setup| setup.welcomed)
    }

    /// Says the first run with this box is done (`true`), or to be had again (`false`); answers
    /// how the box has it now.
    pub fn set_welcomed(&self, welcomed: bool) -> Result<bool> {
        self.value::<Welcomed>(&Request::post("kuvo/setup/welcomed", json!({ "welcomed": welcomed }))).map(|said| said.welcomed)
    }
}
