//! Rust implementation of `com.mbreissi.edgecommons.ConfigComponent`.
//!
//! The crate keeps catalog parsing, source access, promotion, and message serving
//! separate so the v1 file source can be replaced later without changing request
//! handling.

pub mod bootstrap;
pub mod catalog;
pub mod coordinator;
pub mod server;
pub mod source;
pub mod tokens;
