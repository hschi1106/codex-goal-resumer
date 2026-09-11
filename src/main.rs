mod monitor;
mod status;
mod tmux;

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;

use crate::monitor::MonitorConfig;
use crate::tmux::Tmux;

#[derive(Debug, Parser)]
#[command(name = "codex-goal-resumer", version, about)]
struct Cli {
    /// Project directory (defaults to the current working directory)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// tmux session name
    #[arg(long, default_value = "codex-goal-resumer")]
    session: String,

    /// Seconds between checks while Codex is running
    #[arg(long, default_value_t = 20)]
    watch_interval: u64,

    /// Maximum seconds between quota checks while limited
    #[arg(long, default_value_t = 1800)]
    poll_interval: u64,

    /// Seconds to wait beyond a parsed reset time
    #[arg(long, default_value_t = 10)]
    reset_grace: u64,

    /// Number of /status refreshes per quota check
    #[arg(long, default_value_t = 3)]
    status_refresh_count: u32,

    /// Seconds after each /status refresh
    #[arg(long, default_value_t = 3)]
    status_refresh_delay: u64,

    /// Codex executable; --yolo is always added
    #[arg(long, default_value = "codex")]
    codex_bin: PathBuf,

    /// Resume a specific Codex conversation by UUID or session name
    #[arg(long, value_name = "SESSION_ID_OR_NAME")]
    resume: Option<String>,

    #[arg(long, hide = true)]
    internal_watchdog: bool,

    #[arg(long, hide = true, requires = "internal_watchdog")]
    pane_id: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    validate_config(&cli)?;

    if cli.internal_watchdog {
        let pane_id = cli
            .pane_id
            .as_deref()
            .context("internal watchdog requires --pane-id")?;
        return monitor::run(
            Tmux::new(pane_id),
            MonitorConfig {
                watch_interval: Duration::from_secs(cli.watch_interval),
                poll_interval: Duration::from_secs(cli.poll_interval),
                reset_grace: Duration::from_secs(cli.reset_grace),
                status_refresh_count: cli.status_refresh_count,
                status_refresh_delay: Duration::from_secs(cli.status_refresh_delay),
            },
        );
    }

    ensure_program("tmux", &["-V"], "tmux is required but was not found")?;
    ensure_program(&cli.codex_bin, &["--version"], "Codex CLI was not found")?;

    let project = canonical_project(cli.path.as_deref())?;
    if Tmux::session_exists(&cli.session)? {
        bail!(
            "tmux session {:?} already exists\n\nAttach with:\n  tmux attach -t {}\n\nOr choose another name:\n  codex-goal-resumer --session another-name",
            cli.session,
            shell_quote(&cli.session)
        );
    }

    let codex_bin = resolve_executable(&cli.codex_bin).context("Codex CLI was not found")?;
    let codex_command = codex_command(&codex_bin, cli.resume.as_deref());
    let codex_home = env::var_os("CODEX_HOME");
    Tmux::create_session(
        &cli.session,
        &project,
        &codex_command,
        codex_home.as_deref(),
    )?;

    let setup = (|| -> Result<()> {
        let pane_id = Tmux::pane_id(&format!("{}:codex.0", cli.session))?;
        let executable = env::current_exe().context("unable to locate current executable")?;
        let watchdog_command = watchdog_command(&executable, &cli, &pane_id);
        Tmux::create_window(&cli.session, "watchdog", &project, &watchdog_command)?;
        Tmux::select_window(&format!("{}:codex", cli.session))?;
        Ok(())
    })();

    if setup.is_err() {
        let _ = Tmux::kill_session(&cli.session);
    }
    setup?;

    println!(
        "Started Codex in tmux session {:?}. Detach with Ctrl-b d.",
        cli.session
    );
    Tmux::attach(&cli.session)
}

fn validate_config(cli: &Cli) -> Result<()> {
    if cli.watch_interval == 0 {
        bail!("--watch-interval must be greater than zero");
    }
    if cli.poll_interval == 0 {
        bail!("--poll-interval must be greater than zero");
    }
    if cli.status_refresh_count == 0 {
        bail!("--status-refresh-count must be greater than zero");
    }
    Ok(())
}

fn canonical_project(path: Option<&Path>) -> Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => env::current_dir().context("unable to read current directory")?,
    };
    let canonical = path
        .canonicalize()
        .with_context(|| format!("project directory does not exist: {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("project path is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn ensure_program(program: impl AsRef<Path>, args: &[&str], message: &str) -> Result<()> {
    match Command::new(program.as_ref()).args(args).output() {
        Ok(output) if output.status.success() => Ok(()),
        _ => bail!(message.to_owned()),
    }
}

fn resolve_executable(program: &Path) -> Result<PathBuf> {
    if program.components().count() > 1 {
        return program
            .canonicalize()
            .with_context(|| format!("unable to resolve {}", program.display()));
    }

    let path = env::var_os("PATH").context("PATH is not set")?;
    env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
        .with_context(|| format!("unable to resolve {} in PATH", program.display()))
}

fn codex_command(executable: &Path, conversation: Option<&str>) -> String {
    let mut command = format!("{} --yolo", shell_quote_path(executable));
    if let Some(conversation) = conversation {
        command.push_str(" resume ");
        command.push_str(&shell_quote(conversation));
    }
    command
}

fn watchdog_command(executable: &Path, cli: &Cli, pane_id: &str) -> String {
    let args = [
        "--internal-watchdog".to_owned(),
        "--pane-id".to_owned(),
        pane_id.to_owned(),
        "--session".to_owned(),
        cli.session.clone(),
        "--watch-interval".to_owned(),
        cli.watch_interval.to_string(),
        "--poll-interval".to_owned(),
        cli.poll_interval.to_string(),
        "--reset-grace".to_owned(),
        cli.reset_grace.to_string(),
        "--status-refresh-count".to_owned(),
        cli.status_refresh_count.to_string(),
        "--status-refresh-delay".to_owned(),
        cli.status_refresh_delay.to_string(),
    ];
    std::iter::once(shell_quote_path(executable))
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_conversation_always_uses_yolo() {
        assert_eq!(
            codex_command(Path::new("/usr/bin/codex"), None),
            "'/usr/bin/codex' --yolo"
        );
    }

    #[test]
    fn resumed_conversation_keeps_yolo_and_quotes_identifier() {
        assert_eq!(
            codex_command(Path::new("/usr/bin/codex"), Some("release goal's thread")),
            "'/usr/bin/codex' --yolo resume 'release goal'\\''s thread'"
        );
    }
}
