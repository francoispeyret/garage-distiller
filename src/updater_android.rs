//! APK updates must be installed by Android, never by replacing a running library.
use anyhow::{Result, bail};
pub const RELEASES_URL: &str = "https://github.com/francoispeyret/garage-distiller/releases";
#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: String,
    pub notes: String,
}
pub fn configured() -> bool {
    false
}
pub fn check() -> Result<Option<ReleaseInfo>> {
    Ok(None)
}
pub fn install(_: &ReleaseInfo) -> Result<()> {
    bail!("Installez le nouvel APK avec le programme d’installation Android.")
}
