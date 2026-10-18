//! gitwebsite core: the content model, information architecture, registry, ideas list,
//! media library, AsciiDoc rendering, static build, and publishing.
//!
//! Every front end — the `gw` CLI, the REST server, the MCP server, and the desktop app —
//! drives a [`Site`]. Nothing here knows about any of them.

pub mod config;
pub mod content;
pub mod ideas;
pub mod media;
pub mod model;
pub mod registry;
pub mod render;
pub mod build;
pub mod publish;
pub mod scaffold;
pub mod site;
pub mod store;
pub mod theme;
pub mod userconfig;
pub mod init;
pub mod story;
pub mod enhance;

pub use site::Site;
