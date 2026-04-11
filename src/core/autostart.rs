use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::core::process::{run_hidden_command_output, run_hidden_command_wait};

const TASK_NAME: &str = "Zapret Hub Autostart";
const RUN_KEY_PATH: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const LEGACY_VALUE_NAME: &str = "ZapretHub";

pub(crate) fn is_enabled() -> Result<bool> {
    Ok(query_task_xml()?.is_some())
}

pub(crate) fn set_enabled(enabled: bool) -> Result<()> {
    remove_legacy_run_entry()?;

    if enabled {
        create_scheduled_task()
    } else {
        delete_scheduled_task()
    }
}

fn create_scheduled_task() -> Result<()> {
    let exe_path = current_exe_path()?;
    let task_run = format!("\"{}\" --autostart", exe_path.display());
    let username = current_username()?;

    run_hidden_command_wait(
        "schtasks",
        &[
            "/Create",
            "/TN",
            TASK_NAME,
            "/TR",
            &task_run,
            "/SC",
            "ONLOGON",
            "/RL",
            "HIGHEST",
            "/RU",
            &username,
            "/IT",
            "/F",
        ],
        &working_dir()?,
    )
    .context("failed to create Windows autostart task")
}

fn delete_scheduled_task() -> Result<()> {
    let output = run_hidden_command_output(
        "schtasks",
        &["/Delete", "/TN", TASK_NAME, "/F"],
        &working_dir()?,
    )
    .context("failed to remove Windows autostart task")?;

    if output.status.success() {
        return Ok(());
    }

    if query_task_xml()?.is_none() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("schtasks delete exited with status {:?}", output.status.code())
    };

    anyhow::bail!("{details}");
}

fn query_task_xml() -> Result<Option<String>> {
    let output = run_hidden_command_output(
        "schtasks",
        &["/Query", "/TN", TASK_NAME, "/XML", "ONE"],
        &working_dir()?,
    )
    .context("failed to query Windows autostart task")?;

    if output.status.code() == Some(1) {
        return Ok(None);
    }

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let details = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("schtasks query exited with status {:?}", output.status.code())
        };

        anyhow::bail!("{details}");
    }

    Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()))
}

fn remove_legacy_run_entry() -> Result<()> {
    let _ = run_hidden_command_output(
        "reg",
        &["delete", RUN_KEY_PATH, "/v", LEGACY_VALUE_NAME, "/f"],
        &working_dir()?,
    );

    Ok(())
}

fn current_exe_path() -> Result<PathBuf> {
    env::current_exe().context("failed to locate current executable")
}

fn current_username() -> Result<String> {
    env::var("USERNAME").context("failed to resolve current Windows username")
}

fn working_dir() -> Result<PathBuf> {
    let current_exe = current_exe_path()?;
    let exe_dir = current_exe
        .parent()
        .context("current executable has no parent directory")?;
    Ok(exe_dir.to_path_buf())
}
