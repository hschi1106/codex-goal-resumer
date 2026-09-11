use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};

pub struct Tmux {
    target: String,
}

impl Tmux {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
        }
    }

    pub fn session_exists(session: &str) -> Result<bool> {
        let output = Command::new("tmux")
            .args(["has-session", "-t", session])
            .output()
            .context("failed to run tmux")?;
        Ok(output.status.success())
    }

    pub fn create_session(
        session: &str,
        directory: &Path,
        command: &str,
        codex_home: Option<&OsStr>,
    ) -> Result<()> {
        let mut tmux = Command::new("tmux");
        tmux.args(["new-session", "-d", "-s", session, "-n", "codex", "-c"])
            .arg(directory);
        if let Some(codex_home) = codex_home {
            let mut environment = OsString::from("CODEX_HOME=");
            environment.push(codex_home);
            tmux.args([OsStr::new("-e"), environment.as_os_str()]);
        }
        tmux.arg(command);
        checked(&mut tmux, "unable to create tmux session")?;
        Ok(())
    }

    pub fn create_window(session: &str, name: &str, directory: &Path, command: &str) -> Result<()> {
        checked(
            Command::new("tmux")
                .args(["new-window", "-d", "-t", session, "-n", name, "-c"])
                .arg(directory)
                .arg(command),
            "unable to create watchdog window",
        )?;
        Ok(())
    }

    pub fn pane_id(target: &str) -> Result<String> {
        let output = checked(
            Command::new("tmux").args(["display-message", "-p", "-t", target, "#{pane_id}"]),
            "unable to find Codex pane",
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn select_window(target: &str) -> Result<()> {
        checked(
            Command::new("tmux").args(["select-window", "-t", target]),
            "unable to select Codex window",
        )?;
        Ok(())
    }

    pub fn attach(session: &str) -> Result<()> {
        let status = Command::new("tmux")
            .args(["attach-session", "-t", session])
            .status()
            .context("failed to attach to tmux")?;
        if !status.success() {
            bail!("tmux attach exited with {status}");
        }
        Ok(())
    }

    pub fn kill_session(session: &str) -> Result<()> {
        checked(
            Command::new("tmux").args(["kill-session", "-t", session]),
            "unable to clean up tmux session",
        )?;
        Ok(())
    }

    pub fn pane_exists(&self) -> bool {
        Command::new("tmux")
            .args(["display-message", "-p", "-t", &self.target, "#{pane_id}"])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    pub fn capture_recent(&self) -> Result<String> {
        let output = checked(
            Command::new("tmux").args([
                "capture-pane",
                "-p",
                "-J",
                "-S",
                "-250",
                "-t",
                &self.target,
            ]),
            "unable to capture Codex pane",
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn send_line(&self, line: &str) -> Result<()> {
        checked(
            Command::new("tmux").args(["send-keys", "-t", &self.target, "-l", line]),
            "unable to send input to Codex pane",
        )?;
        checked(
            Command::new("tmux").args(["send-keys", "-t", &self.target, "Enter"]),
            "unable to submit input to Codex pane",
        )?;
        Ok(())
    }
}

fn checked(command: &mut Command, context: &str) -> Result<Output> {
    let output = command.output().with_context(|| context.to_owned())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            bail!("{context}: {}", output.status);
        }
        bail!("{context}: {stderr}");
    }
    Ok(output)
}
