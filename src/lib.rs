//! An opinionated Kaneo MCP server: a few tools that each do one step of a
//! card workflow and enforce its rules, with compact text results.

pub mod api;
pub mod model;
pub mod ops;
pub mod render;
pub mod server;
pub mod workflow;
