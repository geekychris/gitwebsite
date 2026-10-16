#![recursion_limit = "512"]
//! The gitwebsite application layer: shared operations plus the REST, MCP, and integration
//! front ends. The `gw` binary and the desktop app both link this.

pub mod appclient;
pub mod companion;
pub mod companion_cli;
pub mod integrations;
pub mod mcp;
pub mod ops;
pub mod server;
pub mod server_companion;
pub mod setup;
pub mod stories;
pub mod toolcat;
pub mod enhance;
