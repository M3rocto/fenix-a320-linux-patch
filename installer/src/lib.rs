// SPDX-License-Identifier: MIT
//! Fenix A320 Linux compatibility installer.
//!
//! The installer itself is in [`core`] (Flightdeck), [`steam`], [`targets`],
//! [`guide`], [`web`] and [`cli`]. The remaining modules give it the pieces
//! of Python's standard library that its behaviour was defined with, so that
//! state files, messages and the handling of files stay what they were.
pub mod cli;
pub mod core;
pub mod desktop;
pub mod download;
pub mod error;
pub mod fsx;
pub mod guide;
pub mod json;
pub mod process;
pub mod py;
pub mod steam;
pub mod system;
pub mod targets;
pub mod vdf;
pub mod web;
pub mod xml;

pub use error::{Error, Result};

/// The release this installer belongs to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
