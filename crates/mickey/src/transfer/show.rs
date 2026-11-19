//! Showing a transfer while it runs (PRD 0015, #477): a window in the
//! corner (`mickey-ui progress`) once it has taken more than a moment,
//! told how it goes ten times a second, and told how it ended. Its Cancel
//! closes the transfer's connection; what arrived stays, to resume.

use crate::net::secure::Link;
use crate::threads::LockAnyway;
use crate::transfer::progress::{self, Progress, Stage};
use std::io::{BufRead, Write};
use std::process::{ChildStdin, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// A transfer shorter than this shows no window.
const QUIET_FOR: Duration = Duration::from_secs(2);
/// How often the window hears how it goes.
const EVERY: Duration = Duration::from_millis(100);

/// How a transfer ended.
pub enum Ended {
    Done {
        files: usize,
        bytes: u64,
        folder: String,
    },
    Failed(String),
}

/// Watches one transfer; `end` says how it ended.
pub struct Watcher(Sender<Ended>);

impl Watcher {
    /// Starts watching `progress`, a transfer of files from `who`; `stop`
    /// is a handle on its connection, closed if the owner cancels.
    pub fn start(who: String, progress: progress::Shared, stop: Link) -> Watcher {
        let (ended, end) = mpsc::channel();
        crate::threads::spawn("transfer shown", move || {
            let started = Instant::now();
            let mut window: Option<ChildStdin> = None;
            loop {
                match end.recv_timeout(EVERY) {
                    Ok(how) => {
                        if let Some(mut input) = window {
                            let _ = writeln!(input, "{}", ended_line(&how));
                        }
                        return;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if window.is_none() && started.elapsed() >= QUIET_FOR {
                    window = open(&who, &stop);
                }
                if let Some(input) = &mut window {
                    let line = progress_line(&progress.lock_anyway());
                    // The owner closed the window: the transfer goes on.
                    if writeln!(input, "{line}").is_err() {
                        window = None;
                    }
                }
            }
        });
        Watcher(ended)
    }

    pub fn end(self, how: Ended) {
        let _ = self.0.send(how);
    }
}

/// Opens the window, and closes `stop` when it says the owner cancelled;
/// its input, to tell it how the transfer goes.
fn open(who: &str, stop: &Link) -> Option<ChildStdin> {
    let mut child = crate::window::ui_command(&["progress", who])
        .and_then(|mut ui| ui.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn())
        .inspect_err(|err| eprintln!("mickey: cannot show the transfer: {err}"))
        .ok()?;
    let (input, stdout, stop) = (
        child.stdin.take()?,
        child.stdout.take()?,
        stop.try_clone().ok()?,
    );
    crate::threads::spawn("transfer cancel", move || {
        let cancelled = std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .any(|line| line == "cancel");
        if cancelled {
            let _ = stop.shutdown();
        }
        let _ = child.wait();
    });
    Some(input)
}

/// `<stage>\t<done>\t<total>\t<bytes a second>\t<seconds left>`, the last
/// two empty while not known.
fn progress_line(progress: &Progress) -> String {
    let stage = match progress.stage {
        Stage::Queued | Stage::CheckingSpace => "space",
        Stage::Rereading => "rereading",
        Stage::Paused => "paused",
        Stage::Transferring | Stage::Done => "transferring",
    };
    let speed = progress
        .speed()
        .map_or_else(String::new, |speed| format!("{speed:.0}"));
    let left = progress
        .time_left()
        .map_or_else(String::new, |left| left.as_secs().to_string());
    format!(
        "{stage}\t{}\t{}\t{speed}\t{left}",
        progress.done, progress.total
    )
}

/// `done\t<files>\t<bytes>\t<folder>` or `failed\t<why>`, on one line.
fn ended_line(how: &Ended) -> String {
    let flat = |text: &str| text.replace(['\t', '\n', '\r'], " ");
    match how {
        Ended::Done {
            files,
            bytes,
            folder,
        } => format!("done\t{files}\t{bytes}\t{}", flat(folder)),
        Ended::Failed(why) => format!("failed\t{}", flat(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_hears_the_stage_bytes_speed_and_time_left() {
        let start = Instant::now();
        let mut progress = Progress::new(1000);
        progress.stage = Stage::Transferring;
        progress.advance(0, start);
        assert_eq!(progress_line(&progress), "transferring\t0\t1000\t\t");
        progress.advance(100, start + Duration::from_secs(2));
        assert_eq!(progress_line(&progress), "transferring\t100\t1000\t50\t18");
    }

    #[test]
    fn how_it_ended_is_one_line() {
        let done = Ended::Done {
            files: 3,
            bytes: 42,
            folder: "/home/ana/Downloads/Mickey".into(),
        };
        assert_eq!(ended_line(&done), "done\t3\t42\t/home/ana/Downloads/Mickey");
        let failed = Ended::Failed("cannot reach\n192.0.2.7".into());
        assert_eq!(ended_line(&failed), "failed\tcannot reach 192.0.2.7");
    }
}
