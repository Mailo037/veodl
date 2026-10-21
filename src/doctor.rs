use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::backend::{self, find, inspect_backend, resolve_backend, TERMUX_SETUP};
use crate::cancel::Cancel;
use crate::config::load_config;
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths::{self, arch, platform};
use crate::process::{self, Command};
use crate::updater::{default_registry, fetch_latest_version};
use crate::utils::{clean_text, readable_error};
use crate::version::{self, CURRENT};

pub fn doctor_help() -> String {
    format!(
        "veo doctor - diagnose the local setup

Usage:
  veo doctor [options]
  veo doctor fix [options]

Options:
  -o, --output <path>   Output directory to check (default: current directory)
  --offline            Skip network checks
  -h, --help            Show help

Checks the JavaScript runtime, the output directory, the backend cache, yt-dlp,
FFmpeg/FFprobe, reachability of the registry and the yt-dlp release host,
leftover partial downloads, and stale duplicate-detection records. Downloads no
backend and changes nothing but its own probe files and
the backend cache directory. The fix command restores managed tools (downloads
yt-dlp if needed unless --offline), creates the requested output directory, and
checks again. It does not change PATH, overrides or config values.
Missing desktop media tools are downloaded into veo's cache automatically.
On Android/Termux, fix installs missing tools with: {TERMUX_SETUP} -y
With --offline no tools are downloaded and no package manager is run.
Exit status is 0 when no check fails, 1 otherwise.
"
    )
}

const LABEL_WIDTH: usize = 18;

#[derive(Clone, Debug)]
pub struct Check {
    pub level: &'static str,
    pub label: String,
    pub detail: String,
}

pub fn find_probe_version(text: &str) -> Option<String> {
    re!(r"[0-9]{4}\.[0-9]{2}\.[0-9]{2}(?:\.[0-9]+)?").find(&clean_text(text)).map(|found| found.as_str().to_string())
}

/// Run a trusted tool with fixed arguments; never through a shell.
pub fn probe(executable: &Path, args: &[&str]) -> Result<String> {
    let command = Command::new(executable).args(args.iter().copied()).timeout(Duration::from_secs(15), "timed out");
    let mut stdout = String::new();
    let mut stderr = String::new();
    let code = process::run(&command, &Cancel::new(), |stream, data| {
        let target = if stream == process::Stream::Stdout { &mut stdout } else { &mut stderr };
        if target.len() < 4096 {
            target.push_str(&String::from_utf8_lossy(data));
        }
        Ok(())
    })?;
    if code == Some(0) {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        Err(Error::msg(if stderr.trim().is_empty() {
            format!("{} exited with code {}", paths::basename(executable), code.map_or("null".into(), |code| code.to_string()))
        } else {
            stderr.trim().to_string()
        }))
    }
}

fn writable(directory: &Path) -> std::result::Result<(), String> {
    let attempt = || -> Result<()> {
        fs::create_dir_all(directory)?;
        let probe = crate::fsutil::temp_dir(directory, ".veo-write-")?;
        fs::write(probe.join("probe"), "veo")?;
        fs::remove_dir_all(probe)?;
        Ok(())
    };
    attempt().map_err(|error| readable_error(&error))
}

// The nearest existing ancestor decides whether a not-yet-created output
// directory will be creatable, without creating it as a side effect.
fn nearest_existing(directory: &Path) -> Option<PathBuf> {
    let mut current = paths::resolve(directory);
    loop {
        if fs::metadata(&current).is_ok_and(|info| info.is_dir()) {
            return Some(current);
        }
        let parent = paths::dirname(&current);
        if parent == current {
            return None;
        }
        current = parent;
    }
}

pub fn format_report(checks: &[Check]) -> String {
    let failed = checks.iter().filter(|check| check.level == "fail").count();
    let warned = checks.iter().filter(|check| check.level == "warn").count();
    let mut lines = vec![format!("veo {CURRENT} doctor"), String::new()];
    for check in checks {
        lines.push(format!("  {:<4}  {:<LABEL_WIDTH$}{}", check.level, check.label, check.detail));
    }
    lines.push(String::new());
    let plural = |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    lines.push(if failed > 0 {
        format!("{} found{}.", plural(failed, "problem"), if warned > 0 { format!(", {}", plural(warned, "warning")) } else { String::new() })
    } else if warned > 0 {
        format!("No problems found, {}.", plural(warned, "warning"))
    } else {
        "No problems found.".into()
    });
    format!("{}\n", lines.join("\n"))
}

pub fn collect_checks(output: Option<&str>, offline: bool) -> Vec<Check> {
    let mut checks: Vec<Check> = Vec::new();
    let mut push = |level: &'static str, label: &str, detail: String| checks.push(Check { level, label: label.into(), detail });
    let cancel = Cancel::new();
    let (platform, arch) = (platform(), arch());

    match backend::js_runtime() {
        Some((name, path)) => push("ok", "JS runtime", format!("{name} at {} (used by yt-dlp for YouTube)", path.display())),
        None => push("warn", "JS runtime", "none found — YouTube may need Deno or Node.js on PATH (or VEO_JS_RUNTIME=name:path)".into()),
    }

    let report = inspect_backend(&cancel);
    let supported = platform == "android" || report.asset.is_some() || report.yt_dlp.source == "override";
    let platform_detail = if platform == "android" {
        " (Termux system tools)".to_string()
    } else if let Some(asset) = &report.asset {
        format!(" (yt-dlp {asset})")
    } else if report.yt_dlp.source == "override" {
        " (VEO_YT_DLP_PATH)".into()
    } else {
        " — set VEO_YT_DLP_PATH to a trusted executable".into()
    };
    push(if supported { "ok" } else { "fail" }, "Platform", format!("{platform}/{arch}{platform_detail}"));
    for message in &report.errors {
        push("fail", "Environment", message.clone());
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let target = output.map_or_else(|| paths::resolve(&cwd), |output| paths::resolve_from(&cwd, output));
    let existing = nearest_existing(&target);
    let access = existing.as_ref().map_or(Err("no existing parent directory".to_string()), |existing| writable(existing));
    let note = match &existing {
        Some(existing) if *existing == target => String::new(),
        Some(existing) => format!(" (will be created under {})", existing.display()),
        None => String::new(),
    };
    match &access {
        Ok(()) => push("ok", "Output directory", format!("{}{note}", target.display())),
        Err(reason) => push("fail", "Output directory", format!("{}{note} — {reason}", target.display())),
    }
    match writable(&report.directory) {
        Ok(()) => push("ok", "Backend cache", paths::display(&report.directory)),
        Err(reason) => push("fail", "Backend cache", format!("{} — {reason}", report.directory.display())),
    }

    let yt = &report.yt_dlp;
    let yt_path = yt.path.as_ref().map(paths::display).unwrap_or_default();
    if yt.present && !["override", "system"].contains(&yt.source.as_str()) && yt.verified != Some(true) {
        push("fail", "yt-dlp", format!("{yt_path} — SHA-256 verification failed; run veo doctor fix"));
    } else if yt.present {
        let source = match yt.source.as_str() {
            "override" => "VEO_YT_DLP_PATH".to_string(),
            "system" => "system installation, not pinned by veo".into(),
            _ => format!("release {}{}", report.release, if yt.verified == Some(true) { ", SHA-256 verified" } else { "" }),
        };
        let detail = format!("{yt_path} ({source})");
        match probe(yt.path.as_deref().unwrap_or(Path::new("")), &["--version"]) {
            Ok(output) => {
                push("ok", "yt-dlp", format!("{}{detail}", find_probe_version(&output).map(|version| format!("version {version}, ")).unwrap_or_default()))
            }
            Err(error) => push("fail", "yt-dlp", format!("{detail} — could not run it: {}", readable_error(&error))),
        }
    } else if platform == "android" {
        push("fail", "yt-dlp", format!("not found — run veo doctor fix to install automatically (Termux: {TERMUX_SETUP})"));
    } else if supported {
        push("warn", "yt-dlp", format!("not cached yet; downloaded on first download (release {})", report.release));
    }

    if platform == "android" && yt.source != "override" {
        let ejs = crate::tool_setup::has_termux_ejs(&|names| find(names), &cancel).unwrap_or(false);
        push(
            if ejs { "ok" } else { "fail" },
            "YouTube JS",
            if ejs { "yt-dlp-ejs installed".into() } else { "yt-dlp-ejs missing — run veo doctor fix to install automatically".into() },
        );
    }

    for (name, tool) in [("ffmpeg", &report.ffmpeg), ("ffprobe", &report.ffprobe)] {
        let path = tool.path.as_ref().map(paths::display).unwrap_or_default();
        if tool.source == "cache" && tool.verified == Some(false) {
            push("fail", name, format!("{path} — SHA-256 verification failed; run veo doctor fix"));
            continue;
        }
        if !tool.present {
            push(
                "fail",
                name,
                "not found — run veo doctor fix to install automatically, or set VEO_FFMPEG_PATH to a directory containing ffmpeg and ffprobe".into(),
            );
            continue;
        }
        let source = match tool.source.as_str() {
            "override" => "VEO_FFMPEG_PATH",
            "cache" => "managed cache",
            _ => "system installation",
        };
        match probe(tool.path.as_deref().unwrap_or(Path::new("")), &["-version"]) {
            Ok(_) => push("ok", name, format!("{path} ({source}{})", if tool.verified == Some(true) { ", SHA-256 verified" } else { "" })),
            Err(error) => push("fail", name, format!("{path} ({source}) — could not run it: {}", readable_error(&error))),
        }
    }

    // Only relevant when the bundled tools cannot serve this platform.
    let system = (find(&["ffmpeg"]), find(&["ffprobe"]));
    let media_ready = ["ffmpeg", "ffprobe"].iter().all(|name| checks.iter().any(|check| check.label == *name && check.level == "ok"));
    let (level, detail) = match &system {
        (Some(ffmpeg), Some(_)) => ("ok", format!("{} (fallback)", paths::dirname(ffmpeg).display())),
        _ if media_ready => ("ok", "not required — the selected FFmpeg and FFprobe work".to_string()),
        _ if platform == "android" => ("warn", "not on PATH — in Termux run: pkg install ffmpeg".to_string()),
        _ => ("warn", "not on PATH — run veo doctor fix to prepare the bundled tools".to_string()),
    };
    checks.push(Check { level, label: "System FFmpeg".into(), detail });
    let mut push = |level: &'static str, label: &str, detail: String| checks.push(Check { level, label: label.into(), detail });

    let leftover: Vec<String> = fs::read_dir(&target)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(".veo-") && name != ".veo-history")
                .collect()
        })
        .unwrap_or_default();
    if leftover.is_empty() {
        push("ok", "Partial data", "no leftover download folders".into());
    } else {
        push(
            "warn",
            "Partial data",
            format!(
                "{} legacy folder{} in the output directory ({}{}). These are not migrated to the local cache; inspect them before removing them.",
                leftover.len(),
                if leftover.len() == 1 { "" } else { "s" },
                leftover.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
                if leftover.len() > 3 { ", …" } else { "" }
            ),
        );
    }
    let history_dir = target.join(".veo-history");
    match fs::metadata(&history_dir) {
        Err(_) => push("ok", "Download history", "no duplicate-detection records in the output directory".into()),
        Ok(info) if !info.is_dir() => push(
            "fail",
            "Download history",
            format!("{} is not a directory — remove or rename it; downloads cannot save their duplicate-detection records otherwise", history_dir.display()),
        ),
        Ok(_) => {
            let records: Vec<PathBuf> = fs::read_dir(&history_dir)
                .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| paths::extname(path) == ".json").collect())
                .unwrap_or_default();
            let stale = records
                .iter()
                .filter(|file| {
                    let usable = crate::state::read_json_lenient::<Value>(file)
                        .and_then(|record| record.get("files").and_then(Value::as_array).cloned())
                        .is_some_and(|files| {
                            !files.is_empty()
                                && files.iter().all(|file| {
                                    file.as_str().is_some_and(|file| paths::dirname(file) == target && fs::metadata(file).is_ok_and(|info| info.is_file()))
                                })
                        });
                    !usable
                })
                .count();
            let count = records.len();
            if count == 0 {
                push("ok", "Download history", "empty record folder; removed automatically on the next download into this folder".into());
            } else if stale > 0 {
                push(
                    "warn",
                    "Download history",
                    format!(
                        "{stale} of {count} record{} reference{} missing files; removed automatically on the next download into this folder",
                        if count == 1 { "" } else { "s" },
                        if count == 1 { "s" } else { "" }
                    ),
                );
            } else {
                push("ok", "Download history", format!("{count} record{}, all files present", if count == 1 { "" } else { "s" }));
            }
        }
    }
    let downloads = paths::cache_base().join("downloads");
    let retained = fs::read_dir(&downloads)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_dir()) && re!(r"^\.veo-part-[a-f0-9]{24}$").is_match(&entry.file_name().to_string_lossy())
                })
                .count()
        })
        .unwrap_or(0);
    push(if retained > 0 { "warn" } else { "ok" }, "Local downloads", format!("{}: {retained} retained download folder(s). Failed transfers expire after 15 minutes and are cleaned on the next run; unfinished --resume downloads are kept.", downloads.display()));

    match load_config(None) {
        Ok(loaded) => {
            let keys: Vec<String> = loaded.config.keys().cloned().collect();
            push(
                "ok",
                "Config",
                if loaded.exists {
                    format!("{} ({})", loaded.file.display(), if keys.is_empty() { "no defaults set".into() } else { keys.join(", ") })
                } else {
                    format!("{} not present (optional)", loaded.file.display())
                },
            );
            for warning in loaded.warnings {
                push("warn", "Config", warning);
            }
        }
        Err(error) => push("fail", "Config", readable_error(&error)),
    }

    if offline {
        push("warn", "Network", "checks skipped (--offline)".into());
        return checks;
    }
    let registry = default_registry();
    let latest = match fetch_latest_version(&registry, Duration::from_secs(6)) {
        Ok(latest) => {
            push("ok", "npm registry", format!("{registry} reachable"));
            Some(latest)
        }
        Err(error) => {
            push("warn", "npm registry", format!("{registry} unreachable — {}", readable_error(&error)));
            None
        }
    };
    match crate::http::head("https://github.com/yt-dlp/yt-dlp/releases", Duration::from_secs(6)) {
        Ok(response) => {
            let status = response.status().as_u16();
            push(if status < 500 { "ok" } else { "warn" }, "GitHub", format!("yt-dlp release host reachable (HTTP {status})"));
        }
        Err(error) => {
            push("warn", "GitHub", format!("yt-dlp release host unreachable — {} (set VEO_YT_DLP_PATH to use a local backend)", readable_error(&error)))
        }
    }
    if let Some(latest) = latest {
        let newer = version::cmp(&latest, CURRENT).is_gt();
        push(
            if newer { "warn" } else { "ok" },
            "veo version",
            if newer { format!("{CURRENT} — {latest} is available (run: veo up)") } else { format!("{CURRENT} is the latest release") },
        );
    }
    checks
}

pub fn doctor_main(args: &[String], stdout: &Out, stderr: &Out) -> Result<i32> {
    let mut args = crate::output::strip_color_flags(args);
    let fix = args.first().is_some_and(|arg| arg == "fix");
    if fix {
        args.remove(0);
    }
    let (mut offline, mut output) = (false, None);
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--offline" => offline = true,
            "-h" | "--help" => {
                stdout.write(&doctor_help());
                return Ok(0);
            }
            "-o" | "--output" => {
                index += 1;
                output = Some(args.get(index).cloned().ok_or_else(|| Error::msg("--output requires a directory. Run veo doctor --help for usage."))?);
            }
            token => return Err(Error::msg(format!("Unknown option for veo doctor: {}. Run veo doctor --help for usage.", clean_text(token)))),
        }
        index += 1;
    }
    let mut repairs = Vec::new();
    if fix {
        stdout.line("Repairing local setup…");
        match resolve_backend(Cancel::global(), offline, &|message| stdout.line(clean_text(message)), &|_, _| {}) {
            Ok(_) => stdout.line("Backend tools are ready."),
            Err(error) => repairs.push(Check { level: "fail", label: "Repair tools".into(), detail: readable_error(&error) }),
        }
        if let Some(output) = &output {
            if let Err(error) = fs::create_dir_all(paths::resolve(output)) {
                repairs.push(Check { level: "fail", label: "Repair output".into(), detail: readable_error(&error.into()) });
            }
        }
        stdout.write("Checking setup after repairs…\n\n");
    }
    let mut checks = collect_checks(output.as_deref(), offline);
    checks.extend(repairs);
    stdout.write(&format_report(&checks));
    let failed = checks.iter().any(|check| check.level == "fail");
    if failed {
        stderr.write("veo: some checks failed. See the report above.\n");
    }
    Ok(i32::from(failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_reports() {
        let checks =
            vec![Check { level: "ok", label: "yt-dlp".into(), detail: "x".into() }, Check { level: "warn", label: "Network".into(), detail: "skipped".into() }];
        let report = format_report(&checks);
        assert!(report.contains("  ok    yt-dlp            x\n"), "{report}");
        assert!(report.ends_with("No problems found, 1 warning.\n"));
        assert_eq!(find_probe_version("yt-dlp 2026.08.19\n").as_deref(), Some("2026.08.19"));
    }
}
