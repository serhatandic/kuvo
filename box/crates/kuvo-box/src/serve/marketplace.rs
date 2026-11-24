//! Peer bridge for Marketplace routes inside `kuvo-box`: lists setup computers and forwards
//! shared mutations / agent queries to the setup primary or addressed peer.

use std::collections::HashMap;

use kuvo_peers::setup_lists::Reader;
use kuvo_services::marketplace::{
    bots::AgentBotRef,
    routes::{MarketplaceComputer, MarketplacePeers, PeerAct, PeerActFuture},
};
use kuvo_wire::keys::encode;
use serde_json::json;

pub struct PeerBridge {
    peers: Reader,
}

impl PeerBridge {
    pub fn new(peers: Reader) -> Self {
        Self { peers }
    }
}

impl MarketplacePeers for PeerBridge {
    fn computers(&self, here_name: &str) -> Vec<MarketplaceComputer> {
        let view = self.peers.view();
        let now = self.peers.now();
        let local = view.roster.local();
        let mut list = vec![MarketplaceComputer {
            key: local.id,
            name: if here_name.is_empty() { local.name } else { here_name.into() },
            here: true,
            available: true,
            supports_skill_removal: true,
        }];
        for peer in &view.roster.members {
            let online = view.relay_connected && !view.standing(peer.key, now).away();
            list.push(MarketplaceComputer {
                key: encode(&peer.key),
                name: peer.name.clone(),
                here: false,
                available: online && view.advertises(&peer.key, "marketplace"),
                supports_skill_removal: view.advertises(&peer.key, "marketplace.skills.remove"),
            });
        }
        list
    }

    fn peer_bots(&self) -> HashMap<String, Vec<AgentBotRef>> {
        HashMap::new()
    }

    fn forward<'a>(&'a self, _: &'a str, _: PeerAct) -> PeerActFuture<'a> {
        Box::pin(async {
            (503, json!({ "message": "Marketplace changes are waiting for the main computer." }))
        })
    }
}
