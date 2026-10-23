use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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
                source: "переменная ZAPRET_HUB_BUNDLE_DIR",
            };
        }
    }

    let portable_bundle = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("bundle")));

    if let Some(path) = &portable_bundle
        && is_valid_bundle_dir(path)
    {
        return ResolvedPaths {
            bundle_dir: path.clone(),
            source: "папка приложения",
        };
    }

    if let Ok(current_dir) = env::current_dir() {
        let repo_bundle = current_dir.join("bundle");
        if is_valid_bundle_dir(&repo_bundle) {
            return ResolvedPaths {
                bundle_dir: repo_bundle,
                source: "рабочая папка",
            };
        }
    }

    // Nothing installed yet: point at the place where "Обновления → Установить" will put it.
    ResolvedPaths {
        bundle_dir: portable_bundle.unwrap_or_else(|| PathBuf::from("bundle")),
        source: "не найден",
    }
}

/// A usable Flowseal bundle: the binaries, the lists and at least one `general*.bat` profile.
/// The `hub` helper folder is created by the app itself, so it is not required.
pub(crate) fn is_valid_bundle_dir(path: &Path) -> bool {
    path.join("bin").join("winws.exe").is_file()
        && path.join("lists").is_dir()
        && has_profile_script(path)
}

pub(crate) fn has_profile_script(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            name.starts_with("general") && name.ends_with(".bat")
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_without_hub_folder_is_valid() {
        let root = env::temp_dir().join(format!("zapret-hub-paths-test-{}", std::process::id()));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("lists")).unwrap();
        fs::write(root.join("bin").join("winws.exe"), b"").unwrap();
        assert!(!is_valid_bundle_dir(&root));

        fs::write(root.join("general (ALT).bat"), b"").unwrap();
        assert!(is_valid_bundle_dir(&root));

        fs::remove_dir_all(root).unwrap();
    }
}
