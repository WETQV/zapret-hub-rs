use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::zapret::bundle::BundleProfile;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ProfileTestMode {
    #[default]
    Standard,
    Dpi,
}

impl ProfileTestMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Standard => "Обычный",
            Self::Dpi => "DPI 16–20 КБ",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileTestRequest {
    pub(crate) mode: ProfileTestMode,
    pub(crate) profiles: Vec<BundleProfile>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileTestRow {
    pub(crate) script_name: String,
    pub(crate) label: String,
    pub(crate) ok: usize,
    pub(crate) errors: usize,
    pub(crate) unsupported: usize,
    pub(crate) ping_ok: usize,
    pub(crate) ping_failed: usize,
    pub(crate) blocked: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileTestReport {
    pub(crate) mode: ProfileTestMode,
    pub(crate) rows: Vec<ProfileTestRow>,
    pub(crate) best_script: Option<String>,
    pub(crate) result_path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) enum ProfileTestEvent {
    Started {
        total: usize,
    },
    ProfileStarted {
        current: usize,
        total: usize,
        label: String,
    },
    CheckStarted {
        label: String,
    },
    ProfileFinished(ProfileTestRow),
    Finished(ProfileTestReport),
    Cancelled,
    Failed(String),
}

pub(crate) fn start(
    bundle_path: PathBuf,
    request: ProfileTestRequest,
    cancelled: Arc<AtomicBool>,
    sender: Sender<ProfileTestEvent>,
) {
    thread::spawn(move || {
        let result = run(&bundle_path, request, &cancelled, &sender);
        match result {
            Ok(report) => {
                let _ = sender.send(ProfileTestEvent::Finished(report));
            }
            Err(_) if cancelled.load(Ordering::Relaxed) => {
                let _ = sender.send(ProfileTestEvent::Cancelled);
            }
            Err(error) => {
                let _ = sender.send(ProfileTestEvent::Failed(error.to_string()));
            }
        }
    });
}

pub(crate) fn preflight(bundle_path: &Path, request: &ProfileTestRequest) -> Result<()> {
    if request.profiles.is_empty() {
        anyhow::bail!("выберите хотя бы один профиль");
    }
    if !bundle_path.join("bin").join("winws.exe").is_file() {
        anyhow::bail!("в bundle не найден bin\\winws.exe");
    }
    let curl = std::process::Command::new("where").arg("curl.exe").output();
    if !curl.is_ok_and(|output| output.status.success()) {
        anyhow::bail!("не найден curl.exe; обновите Windows или добавьте curl в PATH");
    }
    let admin = std::process::Command::new("fltmc").arg("filters").output();
    if !admin.is_ok_and(|output| output.status.success()) {
        anyhow::bail!("перезапустите Zapret Hub от имени администратора");
    }
    Ok(())
}

fn run(
    bundle_path: &Path,
    request: ProfileTestRequest,
    cancelled: &AtomicBool,
    sender: &Sender<ProfileTestEvent>,
) -> Result<ProfileTestReport> {
    preflight(bundle_path, &request)?;
    let _ = sender.send(ProfileTestEvent::Started {
        total: request.profiles.len(),
    });
    let targets = load_targets(bundle_path, request.mode)?;
    let _runtime_guard = TestRuntimeGuard { bundle_path };
    let _ipset_guard = IpsetGuard::prepare(bundle_path, request.mode)?;
    let mut rows = Vec::with_capacity(request.profiles.len());
    for (index, profile) in request.profiles.iter().enumerate() {
        ensure_not_cancelled(cancelled)?;
        let _ = sender.send(ProfileTestEvent::ProfileStarted {
            current: index + 1,
            total: request.profiles.len(),
            label: profile.label().to_owned(),
        });
        start_profile(bundle_path, profile)?;
        wait_with_cancellation(Duration::from_secs(5), cancelled)?;
        let row = run_profile_checks(
            bundle_path,
            profile,
            request.mode,
            &targets,
            cancelled,
            sender,
        )?;
        let _ = sender.send(ProfileTestEvent::ProfileFinished(row.clone()));
        rows.push(row);
    }
    let best_script = best_row(&rows, request.mode).map(|row| row.script_name.clone());
    let result_path = write_report(bundle_path, request.mode, &rows, best_script.as_deref())?;
    Ok(ProfileTestReport {
        mode: request.mode,
        rows,
        best_script,
        result_path,
    })
}

struct TestRuntimeGuard<'a> {
    bundle_path: &'a Path,
}

impl Drop for TestRuntimeGuard<'_> {
    fn drop(&mut self) {
        stop_test_winws(self.bundle_path);
    }
}

#[derive(Clone, Debug)]
struct Target {
    name: String,
    value: String,
    ping_only: bool,
}

struct IpsetGuard {
    path: Option<PathBuf>,
    original: Vec<u8>,
}

impl IpsetGuard {
    fn prepare(bundle_path: &Path, mode: ProfileTestMode) -> Result<Self> {
        if mode != ProfileTestMode::Dpi {
            return Ok(Self {
                path: None,
                original: Vec::new(),
            });
        }
        let path = bundle_path.join("lists").join("ipset-all.txt");
        let original =
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        std::fs::write(&path, b"")
            .with_context(|| format!("failed to switch {} for DPI test", path.display()))?;
        Ok(Self {
            path: Some(path),
            original,
        })
    }
}

impl Drop for IpsetGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::write(path, &self.original);
        }
    }
}

fn load_targets(bundle_path: &Path, mode: ProfileTestMode) -> Result<Vec<Target>> {
    if mode == ProfileTestMode::Dpi {
        return load_dpi_targets();
    }
    let source = bundle_path.join("utils").join("targets.txt");
    let content = std::fs::read_to_string(&source).unwrap_or_default();
    let mut targets = content.lines().filter_map(parse_target).collect::<Vec<_>>();
    if targets.is_empty() {
        targets = default_targets();
    }
    Ok(targets)
}

fn parse_target(line: &str) -> Option<Target> {
    let (name, value) = line.split_once('=')?;
    let value = value.trim().trim_matches('"');
    if name.trim().is_empty() || value.is_empty() || value.starts_with('#') {
        return None;
    }
    let ping_only = value.starts_with("PING:");
    Some(Target {
        name: name.trim().to_owned(),
        value: value.trim_start_matches("PING:").to_owned(),
        ping_only,
    })
}

fn default_targets() -> Vec<Target> {
    [
        ("Discord", "https://discord.com"),
        ("YouTube", "https://www.youtube.com"),
        ("Cloudflare", "https://www.cloudflare.com"),
        ("Cloudflare DNS", "PING:1.1.1.1"),
        ("Google DNS", "PING:8.8.8.8"),
    ]
    .into_iter()
    .filter_map(|(name, value)| parse_target(&format!("{name} = \"{value}\"")))
    .collect()
}

fn load_dpi_targets() -> Result<Vec<Target>> {
    #[derive(serde::Deserialize)]
    struct DpiTarget {
        id: String,
        provider: String,
        country: String,
        host: String,
    }
    let targets: Vec<Target> = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get("https://hyperion-cs.github.io/dpi-checkers/ru/tcp-16-20/suite.v2.json")
        .send()?
        .error_for_status()?
        .json::<Vec<DpiTarget>>()?
        .into_iter()
        .map(|target| Target {
            name: format!("{} / {} ({})", target.country, target.provider, target.id),
            value: target.host,
            ping_only: false,
        })
        .collect();
    if targets.is_empty() {
        anyhow::bail!("DPI-набор не вернул ни одной цели");
    }
    Ok(targets)
}

fn start_profile(bundle_path: &Path, profile: &BundleProfile) -> Result<()> {
    hidden_command("cmd")
        .arg("/C")
        .arg(bundle_path.join(profile.script_name()))
        .current_dir(bundle_path)
        .env("NO_UPDATE_CHECK", "1")
        .status()
        .with_context(|| format!("failed to launch {}", profile.script_name()))?;
    Ok(())
}

fn run_profile_checks(
    bundle_path: &Path,
    profile: &BundleProfile,
    mode: ProfileTestMode,
    targets: &[Target],
    cancelled: &AtomicBool,
    sender: &Sender<ProfileTestEvent>,
) -> Result<ProfileTestRow> {
    let mut row = ProfileTestRow {
        script_name: profile.script_name().to_owned(),
        label: profile.label().to_owned(),
        ok: 0,
        errors: 0,
        unsupported: 0,
        ping_ok: 0,
        ping_failed: 0,
        blocked: 0,
    };
    for batch in targets.chunks(8) {
        ensure_not_cancelled(cancelled)?;
        for target in batch {
            let _ = sender.send(ProfileTestEvent::CheckStarted {
                label: target.name.clone(),
            });
        }
        let mut results = Vec::with_capacity(batch.len());
        thread::scope(|scope| {
            let mut tasks = Vec::with_capacity(batch.len());
            for target in batch {
                tasks.push(scope.spawn(|| run_target(mode, target, bundle_path, cancelled)));
            }
            for task in tasks {
                results.push(task.join().map_err(|_| anyhow!("test worker panicked"))?);
            }
            Ok::<(), anyhow::Error>(())
        })?;
        for result in results {
            merge_target_result(&mut row, result?);
        }
    }
    Ok(row)
}

#[derive(Default)]
struct TargetResult {
    ok: usize,
    errors: usize,
    unsupported: usize,
    ping_ok: usize,
    ping_failed: usize,
    blocked: usize,
}

fn run_target(
    mode: ProfileTestMode,
    target: &Target,
    bundle_path: &Path,
    cancelled: &AtomicBool,
) -> Result<TargetResult> {
    let mut result = TargetResult::default();
    if target.ping_only {
        if ping(&target.value, bundle_path)? {
            result.ping_ok += 1;
        } else {
            result.ping_failed += 1;
        }
        return Ok(result);
    }
    for protocol in ["--http1.1", "--tlsv1.2", "--tlsv1.3"] {
        ensure_not_cancelled(cancelled)?;
        let classification = if mode == ProfileTestMode::Dpi {
            dpi_check(&target.value, protocol, bundle_path)?
        } else {
            standard_check(&target.value, protocol, bundle_path)?
        };
        match classification.as_str() {
            "ok" => result.ok += 1,
            "unsupported" => result.unsupported += 1,
            "blocked" => result.blocked += 1,
            _ => result.errors += 1,
        }
    }
    if mode == ProfileTestMode::Standard {
        let host = target
            .value
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or(&target.value);
        if ping(host, bundle_path)? {
            result.ping_ok += 1;
        } else {
            result.ping_failed += 1;
        }
    }
    Ok(result)
}

fn merge_target_result(row: &mut ProfileTestRow, result: TargetResult) {
    row.ok += result.ok;
    row.errors += result.errors;
    row.unsupported += result.unsupported;
    row.ping_ok += result.ping_ok;
    row.ping_failed += result.ping_failed;
    row.blocked += result.blocked;
}

fn standard_check(url: &str, protocol: &str, bundle_path: &Path) -> Result<String> {
    let output = hidden_command("curl.exe")
        .args([
            "-I",
            "-s",
            "-m",
            "5",
            "-o",
            "NUL",
            "-w",
            "%{http_code}",
            "--show-error",
            protocol,
            url,
        ])
        .current_dir(bundle_path)
        .output()?;
    let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if text.contains("not supported")
        || text.contains("unsupported")
        || output.status.code() == Some(35)
    {
        return Ok("unsupported".to_owned());
    }
    Ok(if output.status.success() {
        "ok"
    } else {
        "error"
    }
    .to_owned())
}

fn dpi_check(host: &str, protocol: &str, bundle_path: &Path) -> Result<String> {
    let payload = bundle_path.join("utils").join("zapret-hub-dpi-payload.bin");
    if !payload.is_file() {
        std::fs::write(&payload, pseudo_random_payload(65_536))?;
    }
    let output = hidden_command("curl.exe")
        .args([
            "--range",
            "0-65535",
            "-m",
            "5",
            "-w",
            "%{http_code} %{size_upload} %{size_download} %{time_total}",
            "-o",
            "NUL",
            "-X",
            "POST",
            "--data-binary",
        ])
        .arg(format!("@{}", payload.display()))
        .arg("-s")
        .arg(protocol)
        .arg(format!("https://{host}"))
        .current_dir(bundle_path)
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("not supported")
        || stderr.contains("unsupported")
        || output.status.code() == Some(35)
    {
        return Ok("unsupported".to_owned());
    }
    let parts = text.split_whitespace().collect::<Vec<_>>();
    let upload = parts
        .get(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let download = parts
        .get(2)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default();
    let elapsed = parts
        .get(3)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_default();
    if upload > 0 && download == 0 && elapsed >= 5.0 && !output.status.success() {
        return Ok("blocked".to_owned());
    }
    Ok(if output.status.success() {
        "ok"
    } else {
        "error"
    }
    .to_owned())
}

fn pseudo_random_payload(size: usize) -> Vec<u8> {
    let mut state = 0x7f4a_7c15_u64;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn ping(host: &str, bundle_path: &Path) -> Result<bool> {
    Ok(hidden_command("ping")
        .args(["-n", "3", "-w", "1000", host])
        .current_dir(bundle_path)
        .status()?
        .success())
}

fn stop_test_winws(bundle_path: &Path) {
    let _ = hidden_command("taskkill")
        .args(["/IM", "winws.exe", "/T", "/F"])
        .current_dir(bundle_path)
        .status();
}

fn wait_with_cancellation(duration: Duration, cancelled: &AtomicBool) -> Result<()> {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        ensure_not_cancelled(cancelled)?;
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn ensure_not_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(anyhow!("cancelled"));
    }
    Ok(())
}

fn best_row(rows: &[ProfileTestRow], mode: ProfileTestMode) -> Option<&ProfileTestRow> {
    rows.iter().min_by(|left, right| match mode {
        ProfileTestMode::Standard => right
            .ok
            .cmp(&left.ok)
            .then(left.errors.cmp(&right.errors))
            .then(right.ping_ok.cmp(&left.ping_ok))
            .then(left.script_name.cmp(&right.script_name)),
        ProfileTestMode::Dpi => right
            .ok
            .cmp(&left.ok)
            .then(left.blocked.cmp(&right.blocked))
            .then(left.errors.cmp(&right.errors))
            .then(left.script_name.cmp(&right.script_name)),
    })
}

fn write_report(
    bundle_path: &Path,
    mode: ProfileTestMode,
    rows: &[ProfileTestRow],
    best: Option<&str>,
) -> Result<PathBuf> {
    let results_dir = bundle_path.join("utils").join("test results");
    std::fs::create_dir_all(&results_dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let path = results_dir.join(format!("test_results_{stamp}.txt"));
    let mut content = format!(
        "Zapret Hub native profile test ({})\n\n=== ANALYTICS ===\n",
        mode.label()
    );
    for row in rows {
        let line = if mode == ProfileTestMode::Standard {
            format!(
                "{} : HTTP OK: {}, ERR: {}, UNSUP: {}, Ping OK: {}, Fail: {}",
                row.script_name, row.ok, row.errors, row.unsupported, row.ping_ok, row.ping_failed
            )
        } else {
            format!(
                "{} : OK: {}, FAIL: {}, UNSUP: {}, BLOCKED: {}",
                row.script_name, row.ok, row.errors, row.unsupported, row.blocked
            )
        };
        content.push_str(&line);
        content.push('\n');
    }
    content.push_str(&format!("Best strategy: {}\n", best.unwrap_or("")));
    std::fs::write(&path, content)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

#[cfg(target_os = "windows")]
fn hidden_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(not(target_os = "windows"))]
fn hidden_command(program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_bundle() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zapret-hub-profile-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("lists")).expect("create lists");
        root
    }

    #[test]
    fn parses_ping_target() {
        let target = parse_target("Cloudflare = \"PING:1.1.1.1\"").unwrap();
        assert!(target.ping_only);
        assert_eq!(target.value, "1.1.1.1");
    }
    #[test]
    fn ranks_standard_by_success_then_error() {
        let rows = vec![
            ProfileTestRow {
                script_name: "a".into(),
                label: "A".into(),
                ok: 3,
                errors: 1,
                unsupported: 0,
                ping_ok: 1,
                ping_failed: 0,
                blocked: 0,
            },
            ProfileTestRow {
                script_name: "b".into(),
                label: "B".into(),
                ok: 3,
                errors: 0,
                unsupported: 0,
                ping_ok: 0,
                ping_failed: 1,
                blocked: 0,
            },
        ];
        assert_eq!(
            best_row(&rows, ProfileTestMode::Standard)
                .unwrap()
                .script_name,
            "b"
        );
    }
    #[test]
    fn ranks_dpi_by_blocked_then_errors() {
        let rows = vec![
            ProfileTestRow {
                script_name: "a".into(),
                label: "A".into(),
                ok: 3,
                errors: 0,
                unsupported: 0,
                ping_ok: 0,
                ping_failed: 0,
                blocked: 1,
            },
            ProfileTestRow {
                script_name: "b".into(),
                label: "B".into(),
                ok: 3,
                errors: 1,
                unsupported: 0,
                ping_ok: 0,
                ping_failed: 0,
                blocked: 0,
            },
        ];
        assert_eq!(
            best_row(&rows, ProfileTestMode::Dpi).unwrap().script_name,
            "b"
        );
    }

    #[test]
    fn dpi_ipset_is_restored_when_guard_is_dropped() {
        let root = temporary_bundle();
        let ipset = root.join("lists").join("ipset-all.txt");
        fs::write(&ipset, b"original ipset").expect("write ipset");
        let guard = IpsetGuard::prepare(&root, ProfileTestMode::Dpi).expect("prepare guard");
        assert_eq!(fs::read(&ipset).expect("read cleared ipset"), b"");
        drop(guard);
        assert_eq!(
            fs::read(&ipset).expect("read restored ipset"),
            b"original ipset"
        );
        fs::remove_dir_all(root).expect("remove temporary bundle");
    }

    #[test]
    fn cancellation_is_detected_before_next_check() {
        let cancelled = AtomicBool::new(true);
        assert!(ensure_not_cancelled(&cancelled).is_err());
    }
}
