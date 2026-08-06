//! Steam Launch Options Bot - hardware-aware launch options for installed
//! Steam games.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod codes;
pub mod jsonio;
pub mod proc;
pub mod sysinfo;
pub mod vdf;
