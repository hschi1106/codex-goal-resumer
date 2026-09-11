use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Local};

use crate::status::{contains_usage_limit, parse_latest_status, QuotaState};
use crate::tmux::Tmux;

const RESUME_VERIFY_DELAY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub watch_interval: Duration,
    pub poll_interval: Duration,
    pub reset_grace: Duration,
    pub status_refresh_count: u32,
    pub status_refresh_delay: Duration,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Running,
    Limited,
}

pub fn run(tmux: Tmux, config: MonitorConfig) -> Result<()> {
    log("monitoring Codex");
    let mut state = State::Running;
    let mut tracker = CaptureTracker::default();
    tracker.observe(&tmux.capture_recent()?);

    loop {
        if !tmux.pane_exists() {
            log("Codex pane disappeared; watchdog exiting");
            return Ok(());
        }

        match state {
            State::Running => {
                thread::sleep(config.watch_interval);
                let capture = match tmux.capture_recent() {
                    Ok(capture) => capture,
                    Err(_) if !tmux.pane_exists() => {
                        log("Codex pane disappeared; watchdog exiting");
                        return Ok(());
                    }
                    Err(error) => {
                        log(&format!("unable to inspect Codex pane: {error:#}"));
                        continue;
                    }
                };
                let fresh = tracker.observe(&capture);
                if contains_usage_limit(&fresh) {
                    log("usage limit detected");
                    state = State::Limited;
                }
            }
            State::Limited => {
                let capture = match refresh_status(&tmux, &config) {
                    Ok(capture) => capture,
                    Err(_) if !tmux.pane_exists() => {
                        log("Codex pane disappeared; watchdog exiting");
                        return Ok(());
                    }
                    Err(error) => {
                        log(&format!(
                            "unable to refresh status: {error:#}; retrying in {}",
                            friendly_duration(config.poll_interval)
                        ));
                        sleep_while_pane_exists(&tmux, config.poll_interval);
                        continue;
                    }
                };
                tracker.observe(&capture);
                let now = Local::now().fixed_offset();
                let status = parse_latest_status(&capture, now);
                match status.quota {
                    QuotaState::Available => {
                        log("quota available");
                        match resume_and_verify(&tmux, &mut tracker) {
                            Ok(true) => {
                                log("goal resumed");
                                log("monitoring Codex");
                                state = State::Running;
                            }
                            Ok(false) => {
                                log("goal remains usage limited");
                                sleep_while_pane_exists(&tmux, config.poll_interval);
                            }
                            Err(_) if !tmux.pane_exists() => {
                                log("Codex pane disappeared; watchdog exiting");
                                return Ok(());
                            }
                            Err(error) => {
                                log(&format!(
                                    "unable to verify goal resume: {error:#}; retrying in {}",
                                    friendly_duration(config.poll_interval)
                                ));
                                sleep_while_pane_exists(&tmux, config.poll_interval);
                            }
                        }
                    }
                    QuotaState::Exhausted => {
                        log("quota exhausted");
                        if let Some(reset) = status.next_reset {
                            log(&format!(
                                "next reset: {}",
                                reset.format("%Y-%m-%d %H:%M:%S")
                            ));
                        }
                        let wait = next_wait(now, status.next_reset, &config);
                        let next = now + chrono::Duration::from_std(wait).unwrap_or_default();
                        log(&format!("next check: {}", next.format("%Y-%m-%d %H:%M:%S")));
                        sleep_while_pane_exists(&tmux, wait);
                    }
                    QuotaState::Unknown => {
                        log(&format!(
                            "unable to determine quota status; retrying in {}",
                            friendly_duration(config.poll_interval)
                        ));
                        sleep_while_pane_exists(&tmux, config.poll_interval);
                    }
                }
            }
        }
    }
}

fn refresh_status(tmux: &Tmux, config: &MonitorConfig) -> Result<String> {
    for index in 1..=config.status_refresh_count {
        wait_until_detached(tmux)?;
        log(&format!(
            "refreshing status ({index}/{})",
            config.status_refresh_count
        ));
        tmux.send_line("/status")?;
        thread::sleep(config.status_refresh_delay);
    }
    tmux.capture_recent()
}

fn resume_and_verify(tmux: &Tmux, tracker: &mut CaptureTracker) -> Result<bool> {
    wait_until_detached(tmux)?;
    tracker.observe(&tmux.capture_recent()?);
    log("sending /goal resume");
    tmux.send_line("/goal resume")?;
    thread::sleep(RESUME_VERIFY_DELAY);
    let capture = tmux.capture_recent()?;
    let fresh = tracker.observe(&capture);
    Ok(!contains_usage_limit(&fresh))
}

fn wait_until_detached(tmux: &Tmux) -> Result<()> {
    let mut announced = false;
    while tmux.session_attached()? {
        if !announced {
            log("waiting for tmux clients to detach before sending commands");
            announced = true;
        }
        thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

fn next_wait(
    now: DateTime<chrono::FixedOffset>,
    reset: Option<DateTime<chrono::FixedOffset>>,
    config: &MonitorConfig,
) -> Duration {
    let poll_deadline = now + chrono::Duration::from_std(config.poll_interval).unwrap_or_default();
    let deadline = reset
        .and_then(|reset| {
            chrono::Duration::from_std(config.reset_grace)
                .ok()
                .map(|grace| reset + grace)
        })
        .filter(|reset_deadline| *reset_deadline > now)
        .map_or(poll_deadline, |reset_deadline| {
            reset_deadline.min(poll_deadline)
        });
    (deadline - now).to_std().unwrap_or(Duration::ZERO)
}

fn sleep_while_pane_exists(tmux: &Tmux, duration: Duration) {
    let deadline = Instant::now() + duration;
    while tmux.pane_exists() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        thread::sleep(remaining.min(Duration::from_secs(5)));
    }
}

fn friendly_duration(duration: Duration) -> String {
    if duration.as_secs().is_multiple_of(60) {
        format!("{} minutes", duration.as_secs() / 60)
    } else {
        format!("{} seconds", duration.as_secs())
    }
}

fn log(message: &str) {
    println!("[{}] {message}", Local::now().format("%H:%M:%S"));
}

#[derive(Default)]
struct CaptureTracker {
    previous: Vec<String>,
}

impl CaptureTracker {
    fn observe(&mut self, capture: &str) -> String {
        let current: Vec<String> = capture.lines().map(ToOwned::to_owned).collect();
        if self.previous.is_empty() {
            self.previous = current;
            return String::new();
        }

        let overlap = suffix_prefix_overlap(&self.previous, &current);
        let fresh = if overlap > 0 {
            current[overlap..].join("\n")
        } else {
            let shared = self
                .previous
                .iter()
                .zip(&current)
                .take_while(|(old, new)| old == new)
                .count();
            current[shared..].join("\n")
        };
        self.previous = current;
        fresh
    }
}

fn suffix_prefix_overlap(old: &[String], new: &[String]) -> usize {
    let maximum = old.len().min(new.len());
    (1..=maximum)
        .rev()
        .find(|&length| old[old.len() - length..] == new[..length])
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn tracker_returns_only_scrolled_in_lines() {
        let mut tracker = CaptureTracker::default();
        assert_eq!(tracker.observe("old limit\nworking"), "");
        assert_eq!(tracker.observe("working\nnew output"), "new output");
    }

    #[test]
    fn tracker_does_not_repeat_unchanged_capture() {
        let mut tracker = CaptureTracker::default();
        tracker.observe("usage limit reached");
        assert_eq!(tracker.observe("usage limit reached"), "");
    }

    #[test]
    fn wait_uses_earlier_reset_deadline() {
        let offset = chrono::FixedOffset::east_opt(0).unwrap();
        let now = offset.with_ymd_and_hms(2026, 9, 11, 16, 30, 0).unwrap();
        let reset = offset.with_ymd_and_hms(2026, 9, 11, 16, 37, 0).unwrap();
        let config = MonitorConfig {
            watch_interval: Duration::from_secs(20),
            poll_interval: Duration::from_secs(1800),
            reset_grace: Duration::from_secs(10),
            status_refresh_count: 3,
            status_refresh_delay: Duration::from_secs(3),
        };
        assert_eq!(
            next_wait(now, Some(reset), &config),
            Duration::from_secs(430)
        );
    }

    #[test]
    fn expired_reset_falls_back_to_poll_interval() {
        let offset = chrono::FixedOffset::east_opt(0).unwrap();
        let now = offset.with_ymd_and_hms(2026, 9, 11, 16, 40, 0).unwrap();
        let reset = offset.with_ymd_and_hms(2026, 9, 11, 16, 37, 0).unwrap();
        let config = MonitorConfig {
            watch_interval: Duration::from_secs(20),
            poll_interval: Duration::from_secs(1800),
            reset_grace: Duration::from_secs(10),
            status_refresh_count: 3,
            status_refresh_delay: Duration::from_secs(3),
        };
        assert_eq!(
            next_wait(now, Some(reset), &config),
            Duration::from_secs(1800)
        );
    }
}
