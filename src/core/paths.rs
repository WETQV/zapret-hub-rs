use std::env;
use std::path::PathBuf;

pub(crate) const LEGACY_BUNDLE_PATH: &str =
    r"C:\Users\mejik\Downloads\zapret-discord-youtube-1.9.9c";

#[derive(Clone, Debug)]
pub(crate) struct ResolvedPaths {
    pub(crate) bundle_dir: PathBuf,
    pub(crate) source: &'static str,
}

pub(crate) fn resolve_paths() -> ResolvedPaths {
    if let Ok(bundle_dir) = env::var("ZAPRET_HUB_BUNDLE_DIR") {
        let path = PathBuf::from(bundle_dir);
        if is_valid_bundle_dir(&path) {
            return ResolvedPaths {
                bundle_dir: path,
                source: "env override",
            };
        }
    }

    if let Ok(current_exe) = env::current_exe()
        && let Some(exe_dir) = current_exe.parent()
    {
        let portable_bundle = exe_dir.join("bundle");
        if is_valid_bundle_dir(&portable_bundle) {
            return ResolvedPaths {
                bundle_dir: portable_bundle,
                source: "portable install",
            };
        }
    }

    if let Ok(current_dir) = env::current_dir() {
        let repo_bundle = current_dir.join("bundle");
        if is_valid_bundle_dir(&repo_bundle) {
            return ResolvedPaths {
                bundle_dir: repo_bundle,
                source: "working directory",
            };
        }
    }

    ResolvedPaths {
        bundle_dir: PathBuf::from(LEGACY_BUNDLE_PATH),
        source: "legacy fallback",
    }
}

pub(crate) fn is_valid_bundle_dir(path: &std::path::Path) -> bool {
    path.join("hub").is_dir()
        && path.join("bin").is_dir()
        && path.join("lists").is_dir()
        && path
            .join("hub")
            .join("run_full_simple_fake_alt2.cmd")
            .is_file()
}
