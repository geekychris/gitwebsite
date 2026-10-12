#![recursion_limit = "512"]
//! The gitwebsite application layer: shared operations plus the REST, MCP, and integration
//! front ends. The `gw` binary and the desktop app both link this.

pub mod integrations;
pub mod mcp;
pub mod ops;
pub mod server;
pub mod setup;
pub mod stories;
pub mod enhance;
