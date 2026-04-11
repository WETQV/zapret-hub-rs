use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn run_hidden_batch_wait(script_path: &Path, current_dir: &Path) -> Result<()> {
    let status = hidden_command("cmd")
        .arg("/C")
        .arg(script_path)
        .current_dir(current_dir)
        .status()
        .with_context(|| format!("failed to run {}", script_path.display()))?;

    ensure_success(status.code(), script_path.display().to_string())
}

pub(crate) fn run_hidden_command_wait(
    program: &str,
    args: &[&str],
    current_dir: &Path,
) -> Result<()> {
    let status = hidden_command(program)
        .args(args)
        .current_dir(current_dir)
        .status()
        .with_context(|| format!("failed to run {program}"))?;

    ensure_success(status.code(), program.to_owned())
}

pub(crate) fn run_hidden_command_output(
    program: &str,
    args: &[&str],
    current_dir: &Path,
) -> Result<Output> {
    hidden_command(program)
        .args(args)
        .current_dir(current_dir)
        .output()
        .with_context(|| format!("failed to run {program}"))
}

pub(crate) fn run_visible_batch_detached(script_path: &Path, current_dir: &Path) -> Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", script_path.to_string_lossy().as_ref()])
        .current_dir(current_dir)
        .spawn()
        .with_context(|| format!("failed to launch {}", script_path.display()))?;

    Ok(())
}

pub(crate) fn try_run_hidden_command(program: &str, args: &[&str], current_dir: &Path) {
    let _ = hidden_command(program)
        .args(args)
        .current_dir(current_dir)
        .status();
}

fn hidden_command(program: &str) -> Command {
    let mut command = Command::new(program);

    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

fn ensure_success(code: Option<i32>, target: String) -> Result<()> {
    let success = code == Some(0);
    if success {
        return Ok(());
    }

    anyhow::bail!(
        "command exited with status {}: {}",
        code.map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        target
    );
}
