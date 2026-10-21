//! Command-line entry point: argument parsing and command dispatch.

mod page;
mod parse;
mod run;
mod schema;

use serde_json::{Map, Value};

use crate::config::{apply_profile, load_config};
use crate::error::Result;
use crate::jobs;
use crate::output::{output_options, Out};
use crate::paths;
use crate::terminal::{self, Std};
use crate::utils::readable_error;
use crate::version::CURRENT;

pub use page::*;
pub use parse::*;
use run::*;
pub use schema::*;

/// Help, version, release notes and the update commands report versions
/// themselves; anything else earns the daily update hint on success.
pub fn should_update_notice(args: &[String]) -> bool {
    if args.iter().any(|arg| ["-h", "--help", "-v", "--version"].contains(&arg.as_str())) {
        return false;
    }
    !args.first().is_some_and(|arg| ["update", "up", "upgrade", "check", "version", "help", "changes", "changelog"].contains(&arg.as_str()))
}

pub(super) const SUBCOMMANDS: [&str; 16] = [
    "stats",
    "history",
    "changes",
    "changelog",
    "flush",
    "runs",
    "inspect",
    "stop",
    "update",
    "up",
    "upgrade",
    "check",
    "doctor",
    "backend",
    "alias",
    "uninstall",
];

/// The whole program: returns the process exit code.
pub fn main(args: Vec<String>) -> i32 {
    let mut args = args;
    if args.first().is_some_and(|arg| arg == "help") {
        args[0] = "--help".into();
    }
    if let Some(message) = mistyped_command_option(&args) {
        Std::Err.write(&format!("veo: {message}\n"));
        return 1;
    }
    let json = args.iter().any(|arg| arg == "--json");
    let mut color = !args.iter().any(|arg| arg == "--no-color") && !json;
    let mut loaded_config = Map::new();
    let display = match output_options(&args) {
        Ok(display) => Some(display),
        Err(error) => {
            Std::Err.write(&format!("veo: {}\n", readable_error(&error)));
            return 1;
        }
    };
    let display = display.unwrap();
    match load_config(None) {
        Ok(loaded) => {
            let applied = apply_profile(&loaded.config, display.profile.as_deref());
            color =
                !json && display.color.unwrap_or_else(|| applied.as_ref().ok().and_then(|config| config.get("color").and_then(Value::as_bool)).unwrap_or(true));
            // Validate explicit profiles even when a color flag overrides their setting.
            if let (Some(_), Err(error)) = (&display.profile, &applied) {
                Std::Err.write(&format!("veo: {}\n", readable_error(error)));
                return 1;
            }
            loaded_config = loaded.config;
        }
        Err(error) => {
            if display.profile.is_some() {
                Std::Err.write(&format!("veo: {}\n", readable_error(&error)));
                return 1;
            }
            // The command's own validation reports configuration errors.
        }
    }
    if args.first().is_some_and(|arg| SUBCOMMANDS.contains(&arg.as_str())) {
        args = display.remaining;
    }
    terminal::set_color(color);
    let code = run_main(args.clone());
    // One throttled (once per day) update hint after a successful command.
    let mut incognito = args.is_empty();
    if !incognito && code == 0 && should_update_notice(&args) {
        incognito = parse_cli(&args, &loaded_config).map(|options| options.incognito).unwrap_or(false);
    }
    if code == 0 && should_update_notice(&args) && !incognito {
        if let Some(notice) = crate::updater::maybe_update_notice(CURRENT) {
            Std::Err.write(&format!("{notice}\n"));
        }
    }
    code
}

pub(super) fn report<T>(stderr: &Out, result: Result<T>, success: impl FnOnce(T) -> i32) -> i32 {
    match result {
        Ok(value) => success(value),
        Err(error) => {
            stderr.write(&format!("veo: {}\n", readable_error(&error)));
            if error.is_cancelled() {
                130
            } else {
                1
            }
        }
    }
}

pub(super) fn run_main(mut args: Vec<String>) -> i32 {
    let json = args.iter().any(|arg| arg == "--json");
    let stdout = Out::stdout(json);
    let stderr = Out::stderr(json);
    let cache = paths::cache_base();
    let command = args.first().cloned().unwrap_or_default();
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let identity = |code: i32| code;
    match command.as_str() {
        "stats" => return report(&stderr, crate::stats::stats_main(&rest, &cache), identity),
        "history" => return report(&stderr, crate::history::history_main(&rest, &cache), identity),
        "changes" | "changelog" => return report(&stderr, crate::changes::changes_main(&rest, &Out::command(Std::Out, &rest)), identity),
        "flush" => return report(&stderr, crate::flush::flush_main(&rest, &cache), identity),
        "runs" => return report(&stderr, crate::runs::runs_main(&rest, &cache), identity),
        "inspect" => return report(&stderr, crate::inspect_media::inspect_main(&rest), identity),
        "stop" => return report(&stderr, crate::runs::stop_main(&rest, &cache, crate::runs::STOP_TIMEOUT), identity),
        "retry" => {
            if args.get(1).map(String::as_str) != Some("--last") {
                stderr.write("veo: Usage: veo retry --last [download options]\n");
                return 1;
            }
            match jobs::latest_failed_job(&cache) {
                Ok(job) => args = ["--retry-failed".to_string(), paths::display(job)].into_iter().chain(args.into_iter().skip(2)).collect(),
                Err(error) => {
                    stderr.write(&format!("veo: {}\n", readable_error(&error)));
                    return 1;
                }
            }
        }
        "subs" => {
            if matches!(args.get(1).map(String::as_str), Some("--help" | "-h")) {
                stdout.write("veo subs <url> [--json] [--source <n>] [--profile <name>]\n\nList manual and automatically generated subtitle languages and source formats without downloading media.\n");
                return 0;
            }
            if args.get(1).is_none_or(|arg| arg.starts_with('-')) {
                stderr.write("veo: Usage: veo subs <url> [--json] [--source <n>]\n");
                return 1;
            }
            args = [args[1].clone(), "--list-subs".into()].into_iter().chain(args.into_iter().skip(2)).collect();
        }
        "profile" => {
            return report(
                &stderr,
                crate::config::config_file().and_then(|file| crate::config::profile_main(&rest, &file, &Out::command(Std::Out, &rest))),
                identity,
            )
        }
        _ => {}
    }
    if let Err(error) = crate::download_cache::cleanup(&crate::download_cache::root(), crate::state::now_ms()) {
        stderr.write(&format!("veo: Could not clean expired local downloads: {}\n", readable_error(&error)));
    }
    crate::updater::clean_released_binaries();
    let command = args.first().cloned().unwrap_or_default();
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    let sub_out = Out::command(Std::Out, &args);
    let sub_err = Out::command(Std::Err, &args);
    match command.as_str() {
        "update" | "up" | "upgrade" | "check" => {
            if command == "check" && args.get(1).map(String::as_str) != Some("update") {
                sub_out.write(crate::updater::UPDATE_HELP);
                return 0;
            }
            return report(&stderr, crate::updater::update_main(&args, &sub_out, &sub_err), identity);
        }
        "config" => return report(&stderr, crate::config::config_main(&rest), identity),
        "doctor" => return report(&stderr, crate::doctor::doctor_main(&rest, &sub_out, &sub_err), identity),
        "backend" => return report(&stderr, crate::backend_update::backend_update_main(&rest, &sub_out, &sub_err), identity),
        "alias" => return report(&stderr, crate::alias::alias_main(&rest, &sub_out), identity),
        "uninstall" => return report(&stderr, crate::uninstall::uninstall_main(&rest, &sub_out, &sub_err), identity),
        _ => {}
    }
    run_download(args, &stdout, &stderr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn parse(values: &[&str]) -> Result<Options> {
        parse_cli(&args(values), &Map::new())
    }

    #[test]
    fn parses_common_flags() {
        let options = parse(&["https://example.com/v", "-q", "720p", "-o", "out", "--no-timings", "-N16"]).unwrap();
        assert_eq!(options.quality, "720p");
        assert_eq!(options.output, "out");
        assert!(!options.timings);
        assert_eq!(options.concurrent_fragments, 16);
        assert_eq!(options.url, "https://example.com/v");
        assert_eq!(options.concurrent_downloads, 2);
    }

    #[test]
    fn rejects_bad_input_with_hints() {
        assert!(parse(&["https://example.com/v", "--qualty", "720p"]).unwrap_err().message.contains("Did you mean \"--quality\"?"));
        assert!(parse(&["https://example.com/v", "-q"]).unwrap_err().message.contains("argument missing"));
        assert!(parse(&["https://example.com/v", "--json=1"]).unwrap_err().message.contains("does not take an argument"));
        assert!(parse(&["https://example.com/v", "-o", "-x"]).unwrap_err().message.contains("ambiguous"));
        assert!(parse(&[]).unwrap_err().message.starts_with("Provide at least one video URL"));
        assert!(parse(&["https://example.com/v", "--audio", "-q", "720p"]).unwrap_err().message.contains("--quality is for video"));
        assert!(parse(&["https://example.com/v", "--format", "mp3"]).unwrap_err().message.contains("Use --audio for audio formats."));
        assert!(parse(&["https://example.com/v", "-N", "65"]).is_err());
        assert!(parse(&["https://example.com/v", "--compatible", "--recode"]).is_err());
    }

    #[test]
    fn config_defaults_and_overrides() {
        let config: Map<String, Value> = serde_json::from_str(r#"{"audio": true, "profiles": {"hd": {"quality": "1080p", "audio": false}}}"#).unwrap();
        let options = parse_cli(&args(&["https://example.com/v"]), &config).unwrap();
        assert!(options.audio);
        // An explicit numeric quality overrides an audio-only default.
        let options = parse_cli(&args(&["https://example.com/v", "-q", "720p"]), &config).unwrap();
        assert!(!options.audio);
        let options = parse_cli(&args(&["https://example.com/v", "--profile", "hd"]), &config).unwrap();
        assert_eq!((options.quality.as_str(), options.profile.as_deref()), ("1080p", Some("hd")));
        assert!(parse_cli(&args(&["https://example.com/v", "--profile", "nope"]), &config).is_err());
    }

    #[test]
    fn derived_settings() {
        let options = parse(&["https://example.com/v", "--playlist-items", "1-3", "--sub-langs", "de"]).unwrap();
        assert!(options.playlist && options.subs);
        let options = parse(&["https://example.com/v", "--compatible"]).unwrap();
        assert_eq!(options.format.as_deref(), Some("mp4"));
        let options = parse(&["https://example.com/v", "--source", "2"]).unwrap();
        assert!(!options.playlist && options.source.as_deref() == Some("2"));
        let options = parse(&["https://example.com/v", "--list-qualitys"]).unwrap();
        assert!(options.list_qualities);
        assert!(parse(&["help"]).unwrap().help);
        assert!(parse(&["--help", "all"]).unwrap().help_all);
        assert!(parse(&["-hv"]).unwrap().help);
    }

    #[test]
    fn suggests_commands_and_subcommand_flags() {
        assert_eq!(mistyped_command_option(&args(&["histroy"])).as_deref(), Some("Unknown command \"histroy\". Did you mean \"history\"?"));
        assert_eq!(mistyped_command_option(&args(&["history", "--limt", "3"])).as_deref(), Some("Unknown option \"--limt\". Did you mean \"--limit\"?"));
        assert_eq!(mistyped_command_option(&args(&["https://example.com"])), None);
    }

    #[test]
    fn update_hint_rules() {
        assert!(should_update_notice(&args(&["https://x"])));
        assert!(!should_update_notice(&args(&["changes"])));
        assert!(!should_update_notice(&args(&["--version"])));
    }
}
