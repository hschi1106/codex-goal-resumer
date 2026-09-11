# codex-goal-resumer

`codex-goal-resumer` is a small Rust CLI that keeps long-running Codex `/goal`
tasks moving across usage-limit interruptions.

It starts Codex in a persistent tmux session, watches the visible terminal
output, checks quota status after a usage or rate limit, and sends
`/goal resume` when quota becomes available again.

It is not a Codex replacement, an API client, or a general-purpose process
supervisor. It does not use private APIs, internal databases, or undocumented
Codex state.

> [!WARNING]
> Every Codex process started by this tool runs in YOLO mode. Codex can execute
> commands and modify files without the usual approval or sandbox protections.
> Use this tool only in repositories and environments you trust.

## Features

- Runs Codex in a persistent tmux session.
- Always starts Codex with `--yolo`.
- Starts a new Codex conversation or resumes a specific existing conversation.
- Detects usage-limit and rate-limit messages from recent terminal output.
- Refreshes `/status` three times by default to reduce stale quota readings.
- Evaluates all quota windows shown by Codex, not only the short-term window.
- Parses same-day, dated, cross-year, and selected `try again at` reset times.
- Polls at least every 30 minutes by default, so manual usage resets are found.
- Resumes the existing Goal in the same Codex process with `/goal resume`.
- Never injects `/status` or `/goal resume` while a tmux client is attached.
- Avoids retriggering on old usage-limit messages left in terminal history.
- Uses a synchronous, dependency-light implementation.

## Requirements

- Linux, or a Unix-like macOS environment
- [Rust](https://www.rust-lang.org/tools/install) stable toolchain
- [tmux](https://github.com/tmux/tmux)
- [Codex CLI](https://developers.openai.com/codex/cli/)

Native Windows terminals are not currently supported. WSL may work when tmux
and Codex are installed inside the Linux environment.

## Installation

### Build from source

```bash
git clone https://github.com/hschi1106/codex-goal-resumer.git
cd codex-goal-resumer
cargo build --release
```

The binary is written to:

```text
target/release/codex-goal-resumer
```

### Install with Cargo

From the repository checkout:

```bash
cargo install --path .
```

### Manual installation

```bash
cp target/release/codex-goal-resumer ~/.local/bin/
```

Make sure `~/.local/bin` is included in `PATH`.

## Quick start

Run in the current project:

```bash
cd ~/my-project
codex-goal-resumer
```

Or pass the project directory explicitly:

```bash
codex-goal-resumer ~/my-project
```

The command opens a tmux window containing `codex --yolo`. Start the Goal
yourself from the Codex prompt:

```text
/goal implement the requested feature
```

`codex-goal-resumer` does not create Goals. It only monitors an existing Goal
and resumes it after quota becomes available.

## Resume an existing Codex conversation

Pass the conversation UUID or session name with `--resume`:

```bash
codex-goal-resumer ~/my-project \
    --resume 019c1234-5678-7000-9000-abcdef012345
```

Named Codex sessions are also accepted:

```bash
codex-goal-resumer ~/my-project --resume my-long-running-session
```

This starts:

```text
codex --yolo resume <SESSION_ID_OR_NAME>
```

The conversation is resumed first. If it contains an unfinished Goal, the
watchdog can subsequently send `/goal resume` when quota recovers. The
conversation must be available under the selected `CODEX_HOME`.

The `--resume` option is different from the automatic `/goal resume` action:

- `--resume` selects a previous Codex conversation at process startup.
- `/goal resume` continues the unfinished Goal inside that conversation after
  a quota interruption.

## CLI reference

```text
codex-goal-resumer [OPTIONS] [PATH]
```

| Option | Description | Default |
| --- | --- | --- |
| `PATH` | Project directory | Current directory |
| `--resume <SESSION_ID_OR_NAME>` | Resume a specific Codex conversation | Start a new conversation |
| `--session <NAME>` | tmux session name | `codex-goal-resumer` |
| `--watch-interval <SECONDS>` | Interval while Codex is running | `20` |
| `--poll-interval <SECONDS>` | Maximum interval between quota checks | `1800` |
| `--reset-grace <SECONDS>` | Delay after a parsed reset time | `10` |
| `--status-refresh-count <COUNT>` | `/status` requests per quota check | `3` |
| `--status-refresh-delay <SECONDS>` | Delay after each `/status` request | `3` |
| `--codex-bin <PATH>` | Codex executable; `--yolo` is still added | `codex` |
| `-h`, `--help` | Print help | — |
| `-V`, `--version` | Print version | — |

Duration options use integer seconds.

### Examples

Use the current directory:

```bash
codex-goal-resumer
```

Use another project directory:

```bash
codex-goal-resumer ~/my-project
```

Use a separate tmux session and poll every 15 minutes:

```bash
codex-goal-resumer ~/my-project \
    --session release-goal \
    --poll-interval 900
```

Resume a particular conversation with faster manual-reset detection:

```bash
codex-goal-resumer ~/my-project \
    --resume 019c1234-5678-7000-9000-abcdef012345 \
    --session resumed-goal \
    --poll-interval 300
```

## How it works

```text
Codex Goal
    |
    v
 Running
    |
    | usage limit detected
    v
 Limited
    |
    +-- /status
    +-- /status
    +-- /status
    |
    v
Quota available?
  |          |
  no         yes
  |           |
 wait         v
  |      /goal resume
  +-----------+
```

The tmux session contains two windows:

- `codex` runs the one and only Codex process for the lifecycle.
- `watchdog` runs the monitor and contains timestamped logs.

The monitor has two states:

- **Running:** capture recent output every 20 seconds. No periodic `/status`
  commands are sent while the Goal is working.
- **Limited:** refresh status, parse the newest quota generation, wait, and
  send `/goal resume` when quota is available.

For input safety, the watchdog does not use `tmux send-keys` while any client is
attached to the session. If a limit is detected while you are watching Codex,
the watchdog logs `waiting for tmux clients to detach before sending commands`.
Detach with `Ctrl-b d` to allow automatic status refresh and Goal resumption.
This prevents watchdog commands from sharing Codex's input buffer with text you
are typing.

After sending `/goal resume`, the watchdog waits 15 seconds and inspects new
output. A new usage-limit message keeps the state Limited. Otherwise monitoring
returns to Running. The cycle can repeat any number of times.

### Why refresh `/status` three times?

The human-readable quota display can briefly be stale or partially refreshed.
Each quota cycle therefore performs this sequence by default:

```text
/status -> wait 3 seconds
/status -> wait 3 seconds
/status -> wait 3 seconds
capture and parse the latest status
```

The parser groups repeated quota labels into status generations and uses the
newest generation. An old `0% left` value therefore does not override a later
`100% left` value. The refresh count and delay are configurable.

Limit detection requires an explicit exhausted message such as `usage limit
reached`, `rate limited`, or `you've hit your usage limit`. Informational text
such as `usage limit reset available` or `information on rate limits` does not
trigger the Limited state.

### Multiple quota windows

All quota records in the latest status generation are evaluated. If any
blocking window is at exactly `0% left`, quota remains exhausted. Percentages
are parsed as complete numbers, so `100% left` cannot be mistaken for `0%`.

### Reset and polling logic

When a reset time is available, the next check is:

```text
min(now + poll_interval, reset_time + reset_grace)
```

For example:

- At 14:00 with a 16:37 reset and a 30-minute poll interval, the next check is
  14:30.
- At 16:30 with a 16:37 reset and a 10-second grace period, the next check is
  16:37:10.

If parsing fails, the watchdog falls back to the poll interval. A stale reset
timestamp that is already in the past also falls back to normal polling rather
than creating a tight loop.

Dates without a year are interpreted as the next reasonable future date. For
example, `02 Jan` observed on December 29 is assigned to the next year.

### Manual usage resets

The watchdog never sleeps indefinitely until the displayed reset time. It
refreshes quota at least once per poll interval. With the default 30-minute
interval, a manual usage reset is normally detected within about 30 minutes.

To check every five minutes while Limited:

```bash
codex-goal-resumer --poll-interval 300
```

## tmux usage

Attach to the default session:

```bash
tmux attach -t codex-goal-resumer
```

Detach without stopping Codex:

```text
Ctrl-b d
```

List sessions:

```bash
tmux ls
```

Switch between windows while attached:

```text
Ctrl-b 0    Codex
Ctrl-b 1    watchdog logs
```

Stop both Codex and the watchdog:

```bash
tmux kill-session -t codex-goal-resumer
```

Closing the outer terminal or detaching does not automatically kill the tmux
session. If the Codex pane disappears, the watchdog exits cleanly.

## Configuration and accounts

### `CODEX_HOME`

The tool does not hard-code `CODEX_HOME`. The current value is explicitly
passed to the new tmux session, which avoids stale environment values from an
already-running tmux server.

```bash
CODEX_HOME=~/.codex-alt codex-goal-resumer ~/my-project
```

### Multiple accounts

Use different `CODEX_HOME` values and unique tmux session names:

```bash
CODEX_HOME=~/.codex \
codex-goal-resumer ~/project-a --session goal-a
```

```bash
CODEX_HOME=~/.codex-alt \
codex-goal-resumer ~/project-b --session goal-b
```

## Logging

The watchdog window emits concise timestamped messages:

```text
[15:42:03] usage limit detected
[15:42:03] refreshing status (1/3)
[15:42:06] refreshing status (2/3)
[15:42:09] refreshing status (3/3)
[15:42:12] quota exhausted
[15:42:12] next reset: 2026-09-11 16:37:00
[15:42:12] next check: 2026-09-11 16:12:12
[16:37:19] quota available
[16:37:19] sending /goal resume
[16:37:34] goal resumed
```

## Troubleshooting

### tmux is missing

```bash
which tmux
tmux -V
```

Install tmux with the operating system package manager. The program exits with
`error: tmux is required but was not found` when it cannot run tmux.

### Codex cannot be found

```bash
which codex
codex --version
```

Add Codex to `PATH` or use `--codex-bin /path/to/codex`.

### The tmux session already exists

Existing sessions are never overwritten. Attach to it:

```bash
tmux attach -t codex-goal-resumer
```

Remove it after confirming it is no longer needed:

```bash
tmux kill-session -t codex-goal-resumer
```

Or select another name:

```bash
codex-goal-resumer --session another-name
```

### A conversation cannot be resumed

Confirm that the identifier is accepted by Codex under the same account:

```bash
codex resume <SESSION_ID_OR_NAME>
```

Also verify that `CODEX_HOME` points to the profile that owns the conversation.

### The Goal did not resume

Open the watchdog window and inspect its log. In the Codex window, run `/status`
and confirm that every blocking quota window has a nonzero percentage. If Codex
immediately reports another limit after `/goal resume`, the watchdog correctly
stays Limited.

### A manual reset was not detected yet

The default polling delay is up to 30 minutes. Reduce it if needed:

```bash
codex-goal-resumer --poll-interval 300
```

### Status parsing broke after a Codex update

The parser depends on human-readable Codex CLI output. If that format changes,
the patterns and tests in `src/status.rs` may need an update. Please open an
issue with a sanitized example of the new output.

## Architecture

```text
src/main.rs     CLI, dependency checks, startup, and tmux attachment
src/tmux.rs     tmux command wrappers
src/monitor.rs  Running/Limited state machine and resume verification
src/status.rs   latest-generation quota and reset-time parsing
```

tmux supplies persistence, capture, input injection, and user reconnection.
The project intentionally avoids a custom PTY, terminal emulator, async runtime,
database, daemon, private API, and configuration framework.

## Limitations

- Requires tmux.
- Depends on human-readable Codex output, which may change.
- Manual reset detection has polling latency.
- Targets Linux and Unix-like macOS environments.
- Always runs Codex in YOLO mode.
- Resume verification depends on newly rendered terminal output.
- Does not provide notifications, a web UI, a daemon, or remote control.

## Security

YOLO mode can bypass normal approval and sandbox restrictions. A long-running
Goal may execute unattended and will be resumed automatically. Do not use this
tool in untrusted repositories, unknown third-party projects, or workspaces that
may contain malicious instructions. Review accessible credentials, network
permissions, and writable data before starting it.

## Development

Run the standard checks before submitting a change:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

Unit tests cover quota percentages, multiple windows, newest-status selection,
same-day and weekly resets, cross-year dates, malformed output, stale reset
fallbacks, capture tracking, and safe Codex command construction.

## Contributing

Issues and pull requests are welcome. Keep changes focused on the core workflow:

```text
detect limit -> refresh status -> wait -> resume Goal
```

Please avoid introducing private Codex integrations or unnecessary background
infrastructure.

## License

This project is licensed under the [MIT License](LICENSE).
