//! Signed GitHub releases. Keep all network and installation calls off the UI thread.
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use std::{io::Read, time::Duration};

const REPOSITORY_API: &str =
    "https://api.github.com/repos/francoispeyret/garage-distiller/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/francoispeyret/garage-distiller/releases";

#[derive(Clone, Debug)]
pub struct ReleaseInfo {
    pub version: String,
    pub notes: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
}

pub fn configured() -> bool {
    public_key().is_ok()
}

fn public_key() -> Result<[u8; 32]> {
    parse_public_key(option_env!("GARAGE_DISTILLER_UPDATE_PUBLIC_KEY_HEX").unwrap_or_default())
}

fn parse_public_key(value: &str) -> Result<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!(
            "Cette version ne contient pas de clé de mise à jour valide. Installez une version signée depuis GitHub Releases."
        );
    }
    let mut key = [0_u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)?;
    }
    if key == [0; 32] {
        bail!("Clé de mise à jour nulle : installation refusée.");
    }
    Ok(key)
}

fn target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => bail!(
            "Les mises à jour sont disponibles pour macOS Intel/Apple Silicon et Windows x64."
        ),
    }
}

fn asset_name(version: &str, target: &str) -> String {
    format!("garage-distiller-v{version}-{target}.zip")
}

/// `None` also covers a repository that has no published release yet.
pub fn check() -> Result<Option<ReleaseInfo>> {
    let client = Client::builder()
        .user_agent(concat!("GarageDistiller/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .get(REPOSITORY_API)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .context("Impossible de vérifier les mises à jour GitHub")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        bail!("La limite de requêtes GitHub est atteinte. Réessayez plus tard.");
    }
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        bail!("Réponse GitHub trop volumineuse.");
    }
    let release: GithubRelease = serde_json::from_slice(&bytes)?;
    inspect_release(release, env!("CARGO_PKG_VERSION"), target()?)
}

fn inspect_release(
    release: GithubRelease,
    current_version: &str,
    target: &str,
) -> Result<Option<ReleaseInfo>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .context("Tag de version invalide : préfixe v requis")?;
    let parsed = Version::parse(version)?;
    if !parsed.pre.is_empty() || parsed <= Version::parse(current_version)? {
        return Ok(None);
    }
    let archive = asset_name(version, target);
    if !release.assets.iter().any(|a| a.name == archive)
        || !release.assets.iter().any(|a| a.name == "SHA256SUMS")
    {
        bail!(
            "La version {version} ne contient pas encore l'archive et les sommes de contrôle pour cet ordinateur."
        );
    }
    Ok(Some(ReleaseInfo {
        version: version.to_owned(),
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(16_000)
            .collect(),
    }))
}

/// Verifies the Ed25519 archive signature AND SHA-256 before any replacement.
/// A missing key is an error, never an opt-out from signature verification.
/// The running process continues until the user closes and reopens the app.
pub fn install(release: &ReleaseInfo) -> Result<()> {
    let key = public_key()?;
    let version = Version::parse(&release.version)?;
    if !version.pre.is_empty() || version <= Version::parse(env!("CARGO_PKG_VERSION"))? {
        bail!("Mise à jour refusée : la version doit être stable et plus récente.");
    }
    let target = target()?;
    let mut builder = self_update::backends::github::Update::configure();
    builder
        .repo_owner("francoispeyret")
        .repo_name("garage-distiller")
        .bin_name("garage-distiller")
        .current_version(env!("CARGO_PKG_VERSION"))
        .release_tag(format!("v{version}"))
        .target(target)
        .asset_identifier(asset_name(&release.version, target))
        .verifying_keys(vec![key])
        .checksum_from_asset("SHA256SUMS")
        .verify_release_digest(true)
        .check_install_path_writable(true)
        .timeout(Duration::from_secs(180))
        .no_confirm(true)
        .show_output(false)
        .show_download_progress(false);

    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe()?;
        if exe
            .ancestors()
            .any(|p| p.extension().is_some_and(|ext| ext == "app"))
        {
            // Replace the entire bundle, preserving consistency of resources and code signature.
            builder.bundle_path_in_archive("Garage Distiller.app");
        } else {
            builder.bin_path_in_archive("Garage Distiller.app/Contents/MacOS/garage-distiller");
        }
    }

    builder.build()?.update().context(
        "La mise à jour a échoué. Vérifiez la connexion, la signature et les droits d'écriture du dossier d'installation",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            body: Some("Release notes".into()),
            draft: false,
            prerelease: false,
            assets: vec![
                GithubAsset {
                    name: asset_name(tag.trim_start_matches('v'), "aarch64-apple-darwin"),
                },
                GithubAsset {
                    name: "SHA256SUMS".into(),
                },
            ],
        }
    }

    #[test]
    fn key_is_required_and_must_be_complete() {
        for key in ["", "00", &"0".repeat(64), &"z".repeat(64), &"é".repeat(32)] {
            assert!(parse_public_key(key).is_err());
        }
        assert_eq!(parse_public_key(&"ab".repeat(32)).unwrap(), [0xab; 32]);
    }

    #[test]
    fn unsigned_archive_is_rejected_before_installation() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("unsigned.zip");
        // Valid empty ZIP, deliberately without the Ed25519 signature envelope.
        let mut bytes = [0_u8; 22];
        bytes[..4].copy_from_slice(b"PK\x05\x06");
        std::fs::write(&archive, bytes).unwrap();
        let key =
            parse_public_key("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
                .unwrap();
        assert!(self_update::verify_signature(&archive, &[key]).is_err());
    }

    #[test]
    fn signed_archive_rejects_tampering_wrong_key_and_renaming() {
        // Reproducible empty-ZIP fixture signed with test seed [7; 32], context
        // "signed-fixture.zip". This public test key never signs app releases.
        let hex = "0c0401656432353531397068000001003873624e09d6bcb79354e67c9ad4a18593c115b6521cc9f0627e580efed3297af9ffaa47eba673a7bad41a635e8ebd2a59f743488301ff85c7c29cf7c9262602504b0506000000000000000000000000500000000000";
        let mut bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let key =
            parse_public_key("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
                .unwrap();
        let wrong_key =
            parse_public_key("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
                .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("signed-fixture.zip");
        std::fs::write(&archive, &bytes).unwrap();
        assert!(self_update::verify_signature(&archive, &[key]).is_ok());
        assert!(self_update::verify_signature(&archive, &[wrong_key]).is_err());
        let renamed = temp.path().join("renamed.zip");
        std::fs::write(&renamed, &bytes).unwrap();
        assert!(self_update::verify_signature(&renamed, &[key]).is_err());
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&archive, bytes).unwrap();
        assert!(self_update::verify_signature(&archive, &[key]).is_err());
    }

    #[test]
    fn semver_order_prevents_downgrades_and_compares_numerically() {
        assert!(
            inspect_release(release("v0.9.0"), "0.10.0", "aarch64-apple-darwin")
                .unwrap()
                .is_none()
        );
        assert!(
            inspect_release(release("v0.10.0"), "0.9.0", "aarch64-apple-darwin")
                .unwrap()
                .is_some()
        );
        assert!(
            inspect_release(release("v0.10.0-rc.1"), "0.9.0", "aarch64-apple-darwin")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn releases_require_exact_platform_archive_and_checksums() {
        let mut missing = release("v0.2.0");
        missing.assets.pop();
        assert!(inspect_release(missing, "0.1.0", "aarch64-apple-darwin").is_err());
        assert!(inspect_release(release("v0.2.0"), "0.1.0", "x86_64-pc-windows-msvc").is_err());
        assert!(inspect_release(release("nightly"), "0.1.0", "aarch64-apple-darwin").is_err());
    }
}
