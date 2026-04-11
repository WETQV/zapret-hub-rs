use std::fs;
use std::path::Path;

use serde::Deserialize;

const BUNDLE_VERSION_FILE_NAME: &str = "ZapretBundle.version.json";
const VERSION_PREFIX: &str = "zapret-discord-youtube-";

#[derive(Debug, Deserialize)]
struct BundleVersionFile {
    version: String,
}

pub(crate) fn detect_bundle_version(bundle_path: &Path) -> Option<String> {
    read_bundle_version_file(bundle_path).or_else(|| infer_bundle_version_from_path(bundle_path))
}

fn read_bundle_version_file(bundle_path: &Path) -> Option<String> {
    let version_path = bundle_path.join(BUNDLE_VERSION_FILE_NAME);
    let content = fs::read_to_string(version_path).ok()?;
    let parsed: BundleVersionFile = serde_json::from_str(&content).ok()?;

    Some(parsed.version)
}

fn infer_bundle_version_from_path(bundle_path: &Path) -> Option<String> {
    let folder_name = bundle_path.file_name()?.to_str()?;
    let version = folder_name.strip_prefix(VERSION_PREFIX)?;

    if version.is_empty() {
        return None;
    }

    Some(version.to_owned())
}
