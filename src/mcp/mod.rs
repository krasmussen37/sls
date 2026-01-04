//! SLS MCP Server - Model Context Protocol wrapper for SLS
//!
//! Exposes SLS functionality as MCP tools for AI agent integration.

pub mod server;
pub mod tools;

pub use server::run_mcp_server;
