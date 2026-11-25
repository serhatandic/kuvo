//! Agents that speak ACP, the Agent Client Protocol (Grok's CLI, Antigravity's `agy`): one client
//! for all of them, each a flavour of its own. A process carries several conversations, each one
//! of its sessions, under one holder: conversations started the same way share it.

mod flavour;
mod projection;
mod server;
mod tools;
mod wire;
