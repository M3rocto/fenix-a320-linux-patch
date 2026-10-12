// SPDX-License-Identifier: MIT
//! Fenix A320 Linux compatibility installer.
//!
//! These modules give the installer the pieces of Python's standard library
//! that its behaviour was defined with, so that state files, messages and the
//! handling of files stay what they were.
pub mod error;
pub mod fsx;
pub mod json;
pub mod process;
pub mod py;
pub mod vdf;
pub mod xml;

pub use error::{Error, Result};

/// The release this installer belongs to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
