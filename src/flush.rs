use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{optional, Error, Result};
use crate::output::Out;
use crate::paths;
use crate::runs::{list_runs, wait_for_exit};
use crate::terminal::Std;

pub const FLUSH_HELP: &str = "veo flush [--stats]\n\nStop registered veo runs and remove local downloads (including retained and resume data)\nand retry job JSON files. Saved media, config, history and backend tools are kept.\nStatistics are kept unless --stats is supplied to reset them.\nLocked downloads from older or interrupted processes are skipped.\n";

pub struct Flushed {
    pub stopped: usize,
    pub downloads: usize,
    pub jobs: usize,
    pub skipped: usize,
}

fn children(root: &Path, name: &str) -> Result<Vec<fs::DirEntry>> {
    let directory = root.join(name);
    let Some(info) = optional(fs::symlink_metadata(&directory))? else { return Ok(Vec::new()) };
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(Error::msg(format!("Refusing to clean a redirected cache directory: {}", directory.display())));
    }
    Ok(fs::read_dir(&directory)?.flatten().collect())
}

pub fn flush(root: &Path, timeout: Duration, status: &dyn Fn(&str), stats: bool) -> Result<Flushed> {
    let root = paths::resolve(root);
    fs::create_dir_all(&root)?;
    let gate = root.join(".flush");
    if let Err(error) = fs::create_dir(&gate) {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(Error::msg(format!(
                "Another flush is active, or an interrupted flush left {}. Remove this empty lock directory only when no flush is running.",
                gate.display()
            )));
        }
        return Err(error.into());
    }
    let result = (|| -> Result<Flushed> {
        let mut flushed = Flushed { stopped: 0, downloads: 0, jobs: 0, skipped: 0 };
        let runs = list_runs(&root)?;
        let live: Vec<_> = runs.iter().filter(|run| run.alive).cloned().collect();
        for run in &live {
            fs::write(format!("{}.cancel", run.file.display()), "flush")?;
            flushed.stopped += 1;
        }
        status(&format!("Stopping {} active veo run(s)…", flushed.stopped));
        if !wait_for_exit(&live, timeout) {
            return Err(Error::msg("Some veo runs did not stop in time. No download or job files were removed. Try veo flush again after they stop."));
        }
        for entry in children(&root, "downloads")? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || !re!(r"^\.veo-part-[a-f0-9]{24}$").is_match(&name) {
                continue;
            }
            // Unregistered older versions might still own these locks.
            if fs::symlink_metadata(entry.path().join(".lock")).is_ok() {
                flushed.skipped += 1;
                continue;
            }
            fs::remove_dir_all(entry.path())?;
            flushed.downloads += 1;
        }
        for directory in crate::download_cache::registered_roots(&root)? {
            for entry in fs::read_dir(&directory)?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || !re!(r"^\.veo-part-[a-f0-9]{24}$").is_match(&name) {
                    continue;
                }
                let manifest: Option<serde_json::Value> = crate::state::read_json(&entry.path().join("job.json")).ok().flatten();
                if !manifest
                    .is_some_and(|manifest| manifest["version"] == 1 && manifest["requestKey"].as_str().is_some_and(|key| name == format!(".veo-part-{key}")))
                {
                    continue;
                }
                if fs::symlink_metadata(entry.path().join(".lock")).is_ok() {
                    flushed.skipped += 1;
                    continue;
                }
                fs::remove_dir_all(entry.path())?;
                flushed.downloads += 1;
            }
        }
        for entry in children(&root, "jobs")? {
            if flushed.skipped > 0 {
                continue; // Older runs may still be updating their retry jobs.
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) || !re!(r"^[0-9]+-[a-f0-9-]{36}\.json(?:\.[a-f0-9-]{36}\.tmp)?$").is_match(&name) {
                continue;
            }
            crate::fsutil::remove_file(&entry.path())?;
            flushed.jobs += 1;
        }
        for run in &runs {
            let _ = fs::remove_file(&run.file);
            let _ = fs::remove_file(format!("{}.cancel", run.file.display()));
        }
        if stats {
            crate::stats::reset_stats(&root)?;
        }
        Ok(flushed)
    })();
    let _ = fs::remove_dir(&gate);
    result
}

pub fn flush_main(args: &[String], root: &Path) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(FLUSH_HELP);
        return Ok(0);
    }
    if !args.is_empty() && (args.len() != 1 || args[0] != "--stats") {
        return Err(Error::msg("Usage: veo flush [--stats] (or veo flush --help)"));
    }
    let stats = !args.is_empty();
    let result = flush(root, Duration::from_secs(30), &|text| stdout.line(text), stats)?;
    if stats {
        stdout.line("Statistics reset.");
    }
    stdout.line(format!("Flushed: {} local download folder(s), {} job file(s); {} run(s) stopped.", result.downloads, result.jobs, result.stopped));
    if result.skipped > 0 {
        stdout.line(format!("Skipped {} locked folder(s) and kept retry jobs; check older veo processes before removing their locks.", result.skipped));
    }
    Ok(if result.skipped > 0 { 1 } else { 0 })
}
