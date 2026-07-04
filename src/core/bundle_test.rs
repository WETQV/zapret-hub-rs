use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};

const TEST_SCRIPT_RELATIVE_PATH: &[&str] = &["utils", "test zapret.ps1"];
const RESULTS_RELATIVE_PATH: &[&str] = &["utils", "test results"];
const RESULT_FILE_PREFIX: &str = "test_results_";
const RESULT_FILE_SUFFIX: &str = ".txt";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BundleTestSummary {
    pub(crate) result_path: PathBuf,
    pub(crate) best_strategy: Option<String>,
    pub(crate) configs: Vec<BundleTestConfigSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BundleTestConfigSummary {
    pub(crate) config: String,
    pub(crate) test_type: String,
    pub(crate) analytics: String,
}

pub(crate) fn bundle_test_script_path(bundle_path: &Path) -> PathBuf {
    path_from_segments(bundle_path, TEST_SCRIPT_RELATIVE_PATH)
}

pub(crate) fn start_bundle_test(bundle_path: &Path, _launched_after: SystemTime) -> Result<()> {
    let script_path = bundle_test_script_path(bundle_path);
    if !script_path.is_file() {
        return Err(anyhow!(
            "bundle test script not found: {}",
            script_path.display()
        ));
    }

    Command::new("powershell")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&script_path)
        .current_dir(bundle_path)
        .spawn()
        .with_context(|| format!("failed to launch {}", script_path.display()))?;

    Ok(())
}

pub(crate) fn read_latest_bundle_test_result(
    bundle_path: &Path,
    launched_after: SystemTime,
) -> Result<Option<BundleTestSummary>> {
    let Some(result_path) = latest_result_file(bundle_path, launched_after)? else {
        return Ok(None);
    };

    parse_bundle_test_result_file(&result_path).map(Some)
}

fn latest_result_file(bundle_path: &Path, launched_after: SystemTime) -> Result<Option<PathBuf>> {
    let results_dir = path_from_segments(bundle_path, RESULTS_RELATIVE_PATH);
    if !results_dir.is_dir() {
        return Ok(None);
    }

    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&results_dir)
        .with_context(|| format!("failed to read {}", results_dir.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.starts_with(RESULT_FILE_PREFIX) || !file_name.ends_with(RESULT_FILE_SUFFIX) {
            continue;
        }

        let modified = entry.metadata()?.modified()?;
        if modified < launched_after {
            continue;
        }

        if latest
            .as_ref()
            .is_none_or(|(latest_modified, _)| modified > *latest_modified)
        {
            latest = Some((modified, entry.path()));
        }
    }

    Ok(latest.map(|(_, path)| path))
}

fn parse_bundle_test_result_file(result_path: &Path) -> Result<BundleTestSummary> {
    let content = fs::read_to_string(result_path)
        .with_context(|| format!("failed to read {}", result_path.display()))?;
    Ok(parse_bundle_test_result_content(
        result_path.to_owned(),
        content.trim_start_matches('\u{feff}'),
    ))
}

fn parse_bundle_test_result_content(result_path: PathBuf, content: &str) -> BundleTestSummary {
    let mut configs = Vec::new();
    let mut analytics_lines = Vec::new();
    let mut in_analytics = false;
    let mut best_strategy = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "=== ANALYTICS ===" {
            in_analytics = true;
            continue;
        }

        if let Some(strategy) = trimmed.strip_prefix("Best strategy:") {
            best_strategy = non_empty(strategy.trim());
            continue;
        }

        if in_analytics && !trimmed.is_empty() {
            analytics_lines.push(trimmed.to_owned());
        }
    }

    for line in analytics_lines {
        if let Some((config, analytics)) = line.split_once(" : ") {
            configs.push(BundleTestConfigSummary {
                config: config.trim().to_owned(),
                test_type: analytics_type(analytics).to_owned(),
                analytics: analytics.trim().to_owned(),
            });
        }
    }

    BundleTestSummary {
        result_path,
        best_strategy,
        configs,
    }
}

fn analytics_type(analytics: &str) -> &str {
    if analytics.contains("Ping OK") {
        "standard"
    } else {
        "dpi"
    }
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn path_from_segments(root: &Path, segments: &[&str]) -> PathBuf {
    segments
        .iter()
        .fold(root.to_owned(), |path, segment| path.join(segment))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn parses_standard_result_summary() {
        let summary = parse_bundle_test_result_content(
            PathBuf::from("result.txt"),
            r#"
Config: general (ALT2).bat (Type: standard)

=== ANALYTICS ===
general (ALT2).bat : HTTP OK: 12, ERR: 3, UNSUP: 1, Ping OK: 4, Fail: 1
general (ALT12).bat : HTTP OK: 16, ERR: 0, UNSUP: 0, Ping OK: 5, Fail: 0
Best strategy: general (ALT12).bat
"#,
        );

        assert_eq!(
            summary.best_strategy.as_deref(),
            Some("general (ALT12).bat")
        );
        assert_eq!(summary.configs.len(), 2);
        assert_eq!(summary.configs[0].test_type, "standard");
        assert!(summary.configs[0].analytics.contains("HTTP OK: 12"));
    }

    #[test]
    fn parses_dpi_result_summary() {
        let summary = parse_bundle_test_result_content(
            PathBuf::from("result.txt"),
            r#"
=== ANALYTICS ===
general (ALT3).bat : OK: 9, FAIL: 1, UNSUP: 2, BLOCKED: 0
Best strategy: general (ALT3).bat
"#,
        );

        assert_eq!(summary.best_strategy.as_deref(), Some("general (ALT3).bat"));
        assert_eq!(summary.configs[0].test_type, "dpi");
        assert!(summary.configs[0].analytics.contains("BLOCKED: 0"));
    }

    #[test]
    fn latest_result_ignores_files_older_than_launch() -> Result<()> {
        let root = std::env::temp_dir().join("zapret-hub-test-results");
        let results_dir = path_from_segments(&root, RESULTS_RELATIVE_PATH);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&results_dir)?;
        fs::write(results_dir.join("test_results_old.txt"), "old")?;

        let launched_after = SystemTime::now() + Duration::from_millis(10);
        assert!(latest_result_file(&root, launched_after)?.is_none());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn latest_result_picks_newest_matching_file() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "zapret-hub-test-results-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        let results_dir = path_from_segments(&root, RESULTS_RELATIVE_PATH);
        fs::create_dir_all(&results_dir)?;
        let launched_after = UNIX_EPOCH;
        let first = results_dir.join("test_results_1.txt");
        let second = results_dir.join("test_results_2.txt");
        fs::write(&first, "first")?;
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&second, "second")?;

        assert_eq!(latest_result_file(&root, launched_after)?, Some(second));

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
