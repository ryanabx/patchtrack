//! patchtrack's core: mail in, series and their states out.
//!
//! No I/O happens here. The host (the `patchtrack` binary, and its bridge
//! for the web app) reads files, fetches URLs and keeps the store; the core
//! parses what it hands over and derives everything else, so its tests are
//! bytes in, states out. See ARCHITECTURE.md.

pub mod b4;
pub mod compose;
pub mod difflib;
pub mod evidence;
pub mod local;
pub mod mail;
pub mod mbox;
pub mod news;
pub mod nudge;
pub mod projects;
pub mod rules;
pub mod series;
pub mod settings;
pub mod subject;
pub mod sync;
pub mod thread;
pub mod trees;
pub mod vdiff;
pub mod views;

pub use mail::{Addr, Message};
