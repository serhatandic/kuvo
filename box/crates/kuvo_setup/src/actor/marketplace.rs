//! Setup-wide Marketplace installations (`SharedKind::MarketplaceInstallations`) read/save helper.

use serde_json::{Value, json};

use super::{Command, Owner, Response, SetupHandle};
use crate::{
    Error, Receipt, SharedKind,
    model::{Link, Role},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSetup {
    pub main_here: bool,
    pub primary: String,
    pub sequence: u64,
    pub value: Option<Value>,
}

pub(super) fn accepted(link: &Link) -> Option<MarketplaceSetup> {
    let setup = link.setup().ok()?;
    let sequence = link
        .shared
        .get(&SharedKind::MarketplaceInstallations)
        .map_or(0, |entry| entry.sequence);
    Some(MarketplaceSetup {
        main_here: setup.role == Role::Primary,
        primary: setup.position.primary.clone(),
        sequence,
        value: link.value(SharedKind::MarketplaceInstallations).cloned(),
    })
}

impl SetupHandle {
    pub fn marketplace_setup(&self) -> Option<MarketplaceSetup> {
        self.views.borrow().marketplace.clone()
    }

    pub async fn save_marketplace_installations(
        &self,
        expected: Option<u64>,
        value: Value,
    ) -> Result<Receipt, Error> {
        let response = self
            .request_command(Command::MarketplaceSave { expected, value })
            .await?;
        serde_json::from_value(response.value)
            .map_err(|_| Error::Invalid("marketplace installations receipt"))
    }
}

impl Owner {
    pub(super) async fn save_marketplace_installations(
        &mut self,
        expected: Option<u64>,
        value: Value,
    ) -> Result<Response, Error> {
        let receipt = self
            .save(SharedKind::MarketplaceInstallations, value, expected, None)
            .await?;
        Ok(Response::ok(json!(receipt)))
    }
}
