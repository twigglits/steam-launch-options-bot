//! Steam Launch Options Bot - hardware-aware launch options for installed
//! Steam games.

use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod advisor;
pub mod apply;
pub mod cli;
pub mod codes;
pub mod doctor;
pub mod jsonio;
pub mod proc;
pub mod rules;
pub mod steam;
pub mod sysinfo;
pub mod vdf;

/// The user's home directory.
///
/// Every path this tool touches hangs off it. Python used `Path.home()`, which
/// consults the passwd database when HOME is unset; reading HOME alone keeps
/// the Core free of a libc dependency, and every context this runs in - a
/// shell, a systemd user unit, a test - sets it.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Python's `repr()` for a string, which is what the ported user-facing
/// messages quote with - single quotes, not the double quotes Rust's `{:?}`
/// produces. Several of them are asserted on character-for-character.
pub fn py_repr(value: &str) -> String {
    if value.contains('\'') && !value.contains('"') {
        format!("\"{value}\"")
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}
