//! veo stop: asking runs to stop and waiting for them.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::output::Out;
use crate::terminal::Std;

use super::*;

/// A run answers a stop request by removing its own record, so a run counts
/// as gone as soon as its record disappears.
pub fn wait_for_exit(runs: &[Run], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let pending = runs.iter().any(|run| alive(run.pid) && fs::symlink_metadata(&run.file).is_ok());
        if !pending {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn stop_main(args: &[String], root: &Path, timeout: Duration) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(STOP_HELP);
        return Ok(0);
    }
    if args.len() > 1 {
        return Err(Error::msg("Usage: veo stop [id]"));
    }
    let mut targets = list_runs(root)?;
    if let Some(requested) = args.first() {
        let id = normalize_id(requested)?;
        targets.retain(|run| run.id.as_deref() == Some(id.as_str()));
        if targets.is_empty() {
            return Err(Error::msg(format!("No veo run with id {id} is active. List runs with veo runs.")));
        }
    }
    let (live, stale): (Vec<Run>, Vec<Run>) = targets.into_iter().partition(|run| run.alive);
    for run in &live {
        fs::write(format!("{}.cancel", run.file.display()), "stop")?;
    }
    if !live.is_empty() {
        wait_for_exit(&live, timeout);
    }
    let (remaining, stopped): (Vec<Run>, Vec<Run>) = live.into_iter().partition(|run| fs::symlink_metadata(&run.file).is_ok() && alive(run.pid));
    for run in stale.iter().chain(stopped.iter()) {
        let _ = fs::remove_file(&run.file);
        let _ = fs::remove_file(format!("{}.cancel", run.file.display()));
    }
    let labels = |runs: &[Run]| runs.iter().map(Run::label).collect::<Vec<_>>().join(", ");
    let mut lines = vec!["veo stop".to_string(), String::new()];
    if !stopped.is_empty() {
        lines.push(format!("Stopped: {}", labels(&stopped)));
    }
    if !stale.is_empty() {
        lines.push(format!("Removed {} stale record(s): {}", stale.len(), labels(&stale)));
    }
    if !remaining.is_empty() {
        lines.push(format!("Still running: {}", labels(&remaining)));
    }
    if stopped.is_empty() && stale.is_empty() && remaining.is_empty() {
        lines.push("No veo runs were active.".into());
    }
    if !stopped.is_empty() {
        lines.push(String::new());
        lines.push("Partial data and retry jobs are kept; veo flush removes them.".into());
    }
    stdout.line(lines.join("\n"));
    Ok(if remaining.is_empty() { 0 } else { 1 })
}
