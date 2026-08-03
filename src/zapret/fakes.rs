use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};

const ACTIVE_DISCORD_FILE: &str = "ACTIVE_DISCORD_UDP.bin";
const ACTIVE_GAME_FILE: &str = "ACTIVE_GAME_UDP.bin";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FakeTarget {
    DiscordUdp,
    GameFilterUdp,
}

impl FakeTarget {
    pub(crate) const ALL: [Self; 2] = [Self::DiscordUdp, Self::GameFilterUdp];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DiscordUdp => "Discord Voice UDP",
            Self::GameFilterUdp => "GameFilter UDP",
        }
    }

    fn active_file_name(self) -> &'static str {
        match self {
            Self::DiscordUdp => ACTIVE_DISCORD_FILE,
            Self::GameFilterUdp => ACTIVE_GAME_FILE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FakeCatalogEntry {
    pub(crate) file_name: String,
    digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FakeCatalog {
    pub(crate) entries: Vec<FakeCatalogEntry>,
    pub(crate) discord_current: Option<String>,
    pub(crate) game_current: Option<String>,
}

pub(crate) fn read_catalog(bundle_path: &Path) -> Result<FakeCatalog> {
    let bin_dir = bundle_path.join("bin");
    let mut entries = Vec::new();
    for entry in
        fs::read_dir(&bin_dir).with_context(|| format!("failed to read {}", bin_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || !has_bin_extension(&path) || is_active_file(&path) {
            continue;
        }
        entries.push(FakeCatalogEntry {
            file_name: entry.file_name().to_string_lossy().into_owned(),
            digest: file_digest(&path)?,
        });
    }
    entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));

    Ok(FakeCatalog {
        discord_current: current_name(&bin_dir, FakeTarget::DiscordUdp, &entries)?,
        game_current: current_name(&bin_dir, FakeTarget::GameFilterUdp, &entries)?,
        entries,
    })
}

pub(crate) fn apply_selection(
    bundle_path: &Path,
    target: FakeTarget,
    file_name: &str,
) -> Result<()> {
    let catalog = read_catalog(bundle_path)?;
    let source = catalog
        .entries
        .iter()
        .find(|entry| entry.file_name == file_name)
        .ok_or_else(|| anyhow!("fake file not found: {file_name}"))?;
    let bin_dir = bundle_path.join("bin");
    let source_path = bin_dir.join(&source.file_name);
    let target_path = bin_dir.join(target.active_file_name());
    replace_with_verified_copy(&source_path, &target_path, source.digest)
}

fn current_name(
    bin_dir: &Path,
    target: FakeTarget,
    entries: &[FakeCatalogEntry],
) -> Result<Option<String>> {
    let active = bin_dir.join(target.active_file_name());
    if !active.is_file() {
        return Ok(None);
    }
    let digest = file_digest(&active)?;
    Ok(entries
        .iter()
        .find(|entry| entry.digest == digest)
        .map(|entry| entry.file_name.clone()))
}

fn has_bin_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bin"))
}

fn is_active_file(path: &Path) -> bool {
    path.file_stem().is_some_and(|stem| {
        stem.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("ACTIVE_")
    })
}

fn file_digest(path: &Path) -> Result<[u8; 32]> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Sha256::digest(bytes).into())
}

fn replace_with_verified_copy(
    source: &Path,
    target: &Path,
    expected_digest: [u8; 32],
) -> Result<()> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = target.with_extension(format!("bin.tmp-{stamp}"));
    let backup = target.with_extension(format!("bin.backup-{stamp}"));
    fs::copy(source, &temporary).with_context(|| format!("failed to copy {}", source.display()))?;
    if file_digest(&temporary)? != expected_digest {
        let _ = fs::remove_file(&temporary);
        anyhow::bail!("copied fake file checksum does not match");
    }
    if target.exists() {
        fs::rename(target, &backup)
            .with_context(|| format!("failed to back up {}", target.display()))?;
    }
    if let Err(error) = fs::rename(&temporary, target) {
        let _ = fs::rename(&backup, target);
        return Err(error).with_context(|| format!("failed to activate {}", target.display()));
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_bundle() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zapret-hub-fakes-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("bin")).expect("create fake bundle");
        root
    }

    #[test]
    fn catalog_matches_active_file_by_sha256() {
        let root = temporary_bundle();
        let bin = root.join("bin");
        fs::write(bin.join("discord-a.bin"), b"discord-a").expect("write source");
        fs::write(bin.join("game-b.bin"), b"game-b").expect("write source");
        fs::write(bin.join(ACTIVE_DISCORD_FILE), b"discord-a").expect("write active");
        fs::write(bin.join(ACTIVE_GAME_FILE), b"custom").expect("write active");

        let catalog = read_catalog(&root).expect("read catalog");
        assert_eq!(catalog.discord_current.as_deref(), Some("discord-a.bin"));
        assert_eq!(catalog.game_current, None);
        assert_eq!(catalog.entries.len(), 2);

        fs::remove_dir_all(root).expect("remove fake bundle");
    }

    #[test]
    fn selection_replaces_active_file_after_checksum_verification() {
        let root = temporary_bundle();
        let bin = root.join("bin");
        fs::write(bin.join("voice-a.bin"), b"voice-a").expect("write source");
        fs::write(bin.join("voice-b.bin"), b"voice-b").expect("write source");
        fs::write(bin.join(ACTIVE_DISCORD_FILE), b"voice-a").expect("write active");

        apply_selection(&root, FakeTarget::DiscordUdp, "voice-b.bin").expect("apply selection");
        assert_eq!(
            fs::read(bin.join(ACTIVE_DISCORD_FILE)).expect("read active"),
            b"voice-b"
        );
        assert!(fs::read_dir(&bin).expect("read bin").all(|entry| {
            !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains("backup-")
        }));

        fs::remove_dir_all(root).expect("remove fake bundle");
    }

    #[test]
    fn failed_checksum_keeps_existing_active_file() {
        let root = temporary_bundle();
        let bin = root.join("bin");
        let source = bin.join("source.bin");
        let target = bin.join(ACTIVE_GAME_FILE);
        fs::write(&source, b"new fake").expect("write source");
        fs::write(&target, b"old fake").expect("write active");

        assert!(replace_with_verified_copy(&source, &target, [0; 32]).is_err());
        assert_eq!(fs::read(&target).expect("read active"), b"old fake");

        fs::remove_dir_all(root).expect("remove fake bundle");
    }
}
