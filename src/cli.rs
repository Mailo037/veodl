use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::backend::Backend;
use crate::cancel::Cancel;
use crate::config::{apply_profile, load_config, selected_profile_name};
use crate::downloader::{self, BackendResolver, Deps, Tools};
use crate::error::{Error, Result};
use crate::execution::{real_wait, Adaptive};
use crate::help;
use crate::jobs::{self, JobContext};
use crate::metadata::Metadata;
use crate::naming::validate_template;
use crate::options::Options;
use crate::output::{output_options, Out};
use crate::paths;
use crate::playlist::{describe_estimate, validate_items, Estimate};
use crate::progress::{Reporter, Root, TerminalReporter};
use crate::source_discovery::{self, format_source, parse_source_timeout, should_offer_source_discovery, sources_json, Discovered};
use crate::suggest::suggest;
use crate::terminal::{self, path_rows, Std};
use crate::utils::{
    clean_text, cookie_file_warning, js_trim, readable_error, validate_browser_spec, validate_cookie_file, validate_url, AUDIO_FORMATS, QUALITIES,
    VIDEO_FORMATS,
};
use crate::version::CURRENT;

const STRING_OPTIONS: [(&str, Option<char>); 22] = [
    ("quality", Some('q')),
    ("output", Some('o')),
    ("rename", Some('r')),
    ("format", None),
    ("playlist-concurrency", None),
    ("concurrent-downloads", None),
    ("filename-template", None),
    ("folder-template", None),
    ("cookies", None),
    ("cookies-from-browser", None),
    ("concurrent-fragments", Some('N')),
    ("sub-langs", None),
    ("sub-format", None),
    ("sponsorblock-remove", None),
    ("section", None),
    ("profile", None),
    ("batch-file", None),
    ("retry-failed", None),
    ("playlist-items", None),
    ("source", None),
    ("timeout", None),
    ("", None),
];

const BOOLEAN_OPTIONS: [(&str, Option<char>); 33] = [
    ("experimental-fragments", None),
    ("compatible", None),
    ("recode", None),
    ("adaptive-concurrency", None),
    ("check-space", None),
    ("timings", None),
    ("color", None),
    ("open", None),
    ("audio", None),
    ("resume", None),
    ("incognito", None),
    ("neutral-filename", None),
    ("playlist", None),
    ("subs", None),
    ("auto-subs", None),
    ("list-subs", None),
    ("verify", None),
    ("embed-subs", None),
    ("embed-metadata", None),
    ("embed-thumbnail", None),
    ("closest-quality", None),
    ("list-qualities", None),
    ("list-qualitys", None),
    ("list-formats", None),
    ("list-sources", None),
    ("auto-list-sources", None),
    ("deep-scan", None),
    ("dry-run", None),
    ("json", None),
    ("help", Some('h')),
    ("version", Some('v')),
    ("skip-existing", None),
    ("", None),
];

fn string_options() -> impl Iterator<Item = &'static (&'static str, Option<char>)> {
    STRING_OPTIONS.iter().filter(|(name, _)| !name.is_empty())
}

fn boolean_options() -> impl Iterator<Item = &'static (&'static str, Option<char>)> {
    BOOLEAN_OPTIONS.iter().filter(|(name, _)| !name.is_empty())
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    String,
    Boolean,
}

fn kind_of(name: &str) -> Option<Kind> {
    if string_options().any(|(option, _)| *option == name) {
        Some(Kind::String)
    } else if boolean_options().any(|(option, _)| *option == name) {
        Some(Kind::Boolean)
    } else {
        None
    }
}

fn long_for_short(short: char) -> Option<&'static str> {
    string_options().chain(boolean_options()).find(|(_, letter)| *letter == Some(short)).map(|(name, _)| *name)
}

fn short_and_long(name: &str) -> String {
    match string_options().chain(boolean_options()).find(|(option, _)| *option == name).and_then(|(_, short)| *short) {
        Some(short) => format!("-{short}, --{name}"),
        None => format!("--{name}"),
    }
}

/// Every accepted spelling, for suggestions.
pub fn option_spellings() -> Vec<String> {
    let mut spellings = Vec::new();
    for (name, short) in string_options() {
        spellings.push(format!("--{name}"));
        if let Some(short) = short {
            spellings.push(format!("-{short}"));
        }
    }
    for (name, short) in boolean_options() {
        spellings.push(format!("--{name}"));
        if let Some(short) = short {
            spellings.push(format!("-{short}"));
        }
        spellings.push(format!("--no-{name}"));
    }
    spellings
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Str(String),
    Bool(bool),
}

struct Parsed {
    values: HashMap<String, Val>,
    positionals: Vec<String>,
    typed: HashSet<String>,
}

fn unknown(option: &str) -> Error {
    let suggestion = suggest(option, &option_spellings());
    Error::msg(format!(
        "Unknown option \"{option}\".{}",
        suggestion.map_or(" Run veo --help to see available options.".into(), |suggestion| format!(" Did you mean \"{suggestion}\"?"))
    ))
}

/// Strict argument parsing with `--no-` negation, like Node's util.parseArgs.
fn parse_args(args: &[String], defaults: &HashMap<&'static str, Val>) -> Result<Parsed> {
    let mut values: HashMap<String, Val> = HashMap::new();
    let mut positionals = Vec::new();
    let mut typed = HashSet::new();
    let mut remaining: std::collections::VecDeque<String> = args.iter().cloned().collect();
    let store = |name: &str, raw: &str, value: Option<String>, inline: bool, values: &mut HashMap<String, Val>| -> Result<()> {
        match kind_of(name) {
            Some(Kind::String) => {
                let Some(value) = value else { return Err(Error::msg(format!("Option '{} <value>' argument missing", short_and_long(name)))) };
                if !inline && value.len() > 1 && value.starts_with('-') {
                    let example = if raw.starts_with("--") { format!("'{raw}=-XYZ'") } else { format!("'--{name}=-XYZ' or '{raw}-XYZ'") };
                    return Err(Error::msg(format!("Option '{raw}' argument is ambiguous.\nDid you forget to specify the option argument for '{raw}'?\nTo specify an option argument starting with a dash use {example}.")));
                }
                values.insert(name.to_string(), Val::Str(value));
            }
            Some(Kind::Boolean) => {
                if value.is_some() {
                    return Err(Error::msg(format!("Option '{}' does not take an argument", short_and_long(name))));
                }
                values.insert(name.to_string(), Val::Bool(true));
            }
            None => return Err(unknown(raw)),
        }
        Ok(())
    };
    while let Some(arg) = remaining.pop_front() {
        if arg == "--" {
            positionals.extend(remaining.drain(..));
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            positionals.push(arg);
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (long.to_string(), None),
            };
            if let Some(base) = name.strip_prefix("no-").filter(|base| kind_of(base) == Some(Kind::Boolean)) {
                if inline.is_some() {
                    return Err(Error::msg(format!("Option '{}' does not take an argument", short_and_long(base))));
                }
                values.insert(base.to_string(), Val::Bool(false));
                typed.insert(base.to_string());
                continue;
            }
            if kind_of(&name).is_none() {
                return Err(unknown(&format!("--{name}")));
            }
            let has_inline = inline.is_some();
            let value = if inline.is_some() {
                inline
            } else if kind_of(&name) == Some(Kind::String) {
                remaining.pop_front()
            } else {
                None
            };
            store(&name, &format!("--{name}"), value, has_inline, &mut values)?;
            typed.insert(name);
            continue;
        }
        let chars: Vec<char> = arg.chars().collect();
        let first = chars[1];
        let first_long = long_for_short(first);
        if chars.len() > 2 {
            if first_long.is_some_and(|name| kind_of(name) == Some(Kind::String)) {
                // -q720p: short option with an attached value.
                let name = first_long.unwrap_or_default();
                store(name, &format!("-{first}"), Some(chars[2..].iter().collect()), true, &mut values)?;
                typed.insert(name.to_string());
                continue;
            }
            // A group of short flags: expand -hv to -h -v.
            let mut expanded = Vec::new();
            for (index, short) in chars.iter().enumerate().skip(1) {
                let is_string = long_for_short(*short).is_some_and(|name| kind_of(name) == Some(Kind::String));
                if !is_string || index == chars.len() - 1 {
                    expanded.push(format!("-{short}"));
                } else {
                    expanded.push(format!("-{}", chars[index..].iter().collect::<String>()));
                    break;
                }
            }
            for item in expanded.into_iter().rev() {
                remaining.push_front(item);
            }
            continue;
        }
        let Some(name) = first_long else { return Err(unknown(&arg)) };
        let value = if kind_of(name) == Some(Kind::String) { remaining.pop_front() } else { None };
        store(name, &arg, value, false, &mut values)?;
        typed.insert(name.to_string());
    }
    for (name, value) in defaults {
        values.entry(name.to_string()).or_insert_with(|| value.clone());
    }
    Ok(Parsed { values, positionals, typed })
}

fn config_text(config: &Map<String, Value>, key: &str) -> Option<String> {
    match config.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn config_flag(config: &Map<String, Value>, key: &str, fallback: bool) -> bool {
    match config.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => fallback,
        Some(other) => !matches!(other, Value::String(text) if text.is_empty()) && other.as_f64() != Some(0.0),
    }
}

/// Config defaults, so an explicit flag always wins but a stored preference does not.
fn option_defaults(config: &Map<String, Value>) -> HashMap<&'static str, Val> {
    let mut defaults = HashMap::new();
    let cwd = paths::display(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let strings: [(&str, &str, Option<&str>); 21] = [
        ("quality", "quality", Some("best")),
        ("output", "output", None),
        ("rename", "rename", None),
        ("format", "format", None),
        ("concurrent-downloads", "concurrentDownloads", Some("2")),
        ("filename-template", "filenameTemplate", None),
        ("folder-template", "folderTemplate", None),
        ("playlist-concurrency", "playlistConcurrency", Some("2")),
        ("cookies", "cookies", None),
        ("cookies-from-browser", "cookiesFromBrowser", None),
        ("concurrent-fragments", "concurrentFragments", Some("8")),
        ("sub-langs", "subLangs", None),
        ("sponsorblock-remove", "sponsorblockRemove", None),
        ("section", "section", None),
        ("sub-format", "subFormat", None),
        ("playlist-items", "playlistItems", None),
        ("source", "source", None),
        ("timeout", "timeout", None),
        ("profile", "", None),
        ("batch-file", "", None),
        ("retry-failed", "", None),
    ];
    for (name, key, fallback) in strings {
        if key.is_empty() {
            continue;
        }
        let value = config_text(config, key).or_else(|| fallback.map(str::to_string)).or_else(|| (name == "output").then(|| cwd.clone()));
        if let Some(value) = value {
            defaults.insert(name, Val::Str(value));
        }
    }
    let flags: [(&str, &str, bool); 27] = [
        ("compatible", "compatible", false),
        ("recode", "recode", false),
        ("adaptive-concurrency", "adaptiveConcurrency", true),
        ("check-space", "checkSpace", true),
        ("timings", "timings", true),
        ("color", "color", true),
        ("experimental-fragments", "experimentalFragments", false),
        ("open", "open", false),
        ("audio", "audio", false),
        ("resume", "resume", false),
        ("incognito", "incognito", false),
        ("neutral-filename", "neutralFilename", false),
        ("playlist", "playlist", false),
        ("subs", "subs", false),
        ("auto-subs", "autoSubs", false),
        ("verify", "verify", false),
        ("embed-subs", "embedSubs", false),
        ("embed-metadata", "embedMetadata", false),
        ("embed-thumbnail", "embedThumbnail", false),
        ("closest-quality", "closestQuality", false),
        ("json", "json", false),
        ("skip-existing", "skipExisting", false),
        ("deep-scan", "deepScan", false),
        ("list-sources", "listSources", false),
        ("auto-list-sources", "autoListSources", true),
        ("dry-run", "", false),
        ("list-formats", "", false),
    ];
    for (name, key, fallback) in flags {
        if key.is_empty() {
            continue;
        }
        defaults.insert(name, Val::Bool(config_flag(config, key, fallback)));
    }
    defaults
}

const COMMAND_FLAGS: [(&str, &[&str]); 17] = [
    ("stats", &["--json"]),
    ("history", &["--json", "--limit", "--failed"]),
    ("changes", &["--json", "--version", "--since", "--to", "--range", "--latest"]),
    ("runs", &["--json"]),
    ("stop", &[]),
    ("flush", &["--stats"]),
    ("inspect", &["--json", "--check-audio", "--search"]),
    ("doctor", &["--offline", "--output", "-o"]),
    ("update", &["--check"]),
    ("up", &["--check"]),
    ("upgrade", &["--check"]),
    ("check", &["--check"]),
    ("backend", &["--check", "--keep-files"]),
    ("alias", &["--json", "--force", "--bin-dir"]),
    ("uninstall", &["--prefix", "-p", "--yes", "--keep-cache", "--keep-config", "--keep-aliases", "--json", "--bin-dir"]),
    ("config", &["--profile", "--external", "--terminal"]),
    ("profile", &[]),
];

fn command_names() -> Vec<String> {
    COMMAND_FLAGS.iter().map(|(name, _)| name.to_string()).chain(["changelog", "retry", "subs", "help", "version"].map(str::to_string)).collect()
}

fn mistyped_command_option(args: &[String]) -> Option<String> {
    let command = args.first()?;
    if !command.starts_with('-') && !command_names().contains(command) {
        if let Some(suggestion) = suggest(command, &command_names()) {
            return Some(format!("Unknown command \"{command}\". Did you mean \"{suggestion}\"?"));
        }
    }
    let lookup = if command == "changelog" { "changes" } else { command.as_str() };
    let options: Vec<String> = match COMMAND_FLAGS.iter().find(|(name, _)| *name == lookup) {
        Some((_, flags)) => {
            flags.iter().map(|flag| flag.to_string()).chain(["--help", "-h", "--no-color", "--color", "--profile"].map(str::to_string)).collect()
        }
        None if command == "retry" => option_spellings().into_iter().chain(["--last".to_string()]).collect(),
        None => return None,
    };
    let start = if ["config", "backend", "alias"].contains(&command.as_str()) && args.get(1).is_some_and(|arg| !arg.starts_with('-')) { 2 } else { 1 };
    for arg in args.iter().skip(start) {
        if arg == "--" {
            break;
        }
        let name = arg.split('=').next().unwrap_or("");
        if !name.starts_with('-') || options.iter().any(|option| option == name) {
            continue;
        }
        if let Some(suggestion) = suggest(name, &options) {
            return Some(format!("Unknown option \"{name}\". Did you mean \"{suggestion}\"?"));
        }
    }
    None
}

fn text(values: &HashMap<String, Val>, name: &str) -> Option<String> {
    match values.get(name) {
        Some(Val::Str(text)) => Some(text.clone()),
        _ => None,
    }
}

fn flag(values: &HashMap<String, Val>, name: &str) -> bool {
    matches!(values.get(name), Some(Val::Bool(true)))
}

/// Turn arguments plus config defaults into validated options.
pub fn parse_cli(args: &[String], config: &Map<String, Value>) -> Result<Options> {
    let mut args = args.to_vec();
    if args.first().is_some_and(|arg| arg == "help") {
        args[0] = "--help".into();
    }
    if args.first().is_some_and(|arg| arg == "version") {
        if args.len() != 1 {
            return Err(Error::msg("Usage: veo version"));
        }
        return Ok(Options { version: true, ..Options::default() });
    }
    let preliminary = parse_args(&args, &HashMap::new())?;
    let requested_profile = text(&preliminary.values, "profile");
    let profile_name = selected_profile_name(config, requested_profile.as_deref());
    let config = apply_profile(config, requested_profile.as_deref())?;
    let Parsed { mut values, positionals, typed } = parse_args(&args, &option_defaults(&config))?;
    if flag(&values, "help") {
        return Ok(Options { help: true, help_all: positionals.first().is_some_and(|arg| arg == "all"), ..Options::default() });
    }
    if flag(&values, "version") {
        return Ok(Options { version: true, ..Options::default() });
    }
    let set = |values: &mut HashMap<String, Val>, name: &str, value: Option<Val>| match value {
        Some(value) => {
            values.insert(name.into(), value);
        }
        None => {
            values.remove(name);
        }
    };
    // An explicit numeric --quality states video intent, so it overrides an
    // audio-only default instead of turning into a confusing conflict error.
    let quality = text(&values, "quality").unwrap_or_else(|| "best".into());
    if typed.contains("quality") && quality != "best" && flag(&values, "audio") && !typed.contains("audio") {
        set(&mut values, "audio", Some(Val::Bool(false)));
    }
    let audio = flag(&values, "audio");
    if positionals.is_empty() && text(&values, "batch-file").is_none() && text(&values, "retry-failed").is_none() {
        return Err(Error::msg("Provide at least one video URL. Run veo --help for usage."));
    }
    if !QUALITIES.contains(&quality.as_str()) && !re!(r"^[1-9][0-9]{1,4}p$").is_match(&quality) {
        return Err(Error::msg(format!("Invalid quality. Choose: {} or a numeric resolution.", QUALITIES.join(", "))));
    }
    if let Some(items) = text(&values, "playlist-items") {
        if !flag(&values, "playlist") && typed.contains("playlist") {
            if typed.contains("playlist-items") {
                return Err(Error::msg("--playlist-items cannot be combined with --no-playlist."));
            }
            set(&mut values, "playlist-items", None);
        } else {
            validate_items(&items)?;
            set(&mut values, "playlist", Some(Val::Bool(true)));
        }
    }
    let retry = text(&values, "retry-failed");
    let batch = text(&values, "batch-file");
    if retry.is_some() && (!positionals.is_empty() || batch.is_some()) {
        return Err(Error::msg("--retry-failed cannot be combined with URLs or --batch-file."));
    }
    let incognito = flag(&values, "incognito");
    if incognito && retry.is_some() {
        return Err(Error::msg("--incognito cannot retry a saved job. Use --no-incognito for this request."));
    }
    for (name, message) in
        [("resume", "--incognito cannot be combined with --resume."), ("skip-existing", "--incognito cannot be combined with --skip-existing.")]
    {
        if incognito && flag(&values, name) {
            if typed.contains(name) {
                return Err(Error::msg(message));
            }
            set(&mut values, name, Some(Val::Bool(false)));
        }
    }
    let output = text(&values, "output").unwrap_or_default();
    if js_trim(&output).is_empty() {
        return Err(Error::msg("The output directory cannot be empty."));
    }
    let rename = text(&values, "rename");
    if rename.as_ref().is_some_and(|rename| clean_text(rename).is_empty()) {
        return Err(Error::msg("The custom filename cannot be empty."));
    }
    if !flag(&values, "neutral-filename") && positionals.len() > 1 && rename.as_ref().is_some_and(|rename| !rename.contains('*')) {
        return Err(Error::msg("--rename only applies to a single URL unless the name contains * for the original title."));
    }
    if audio && quality != "best" && typed.contains("quality") {
        return Err(Error::msg("--quality is for video; omit it when using --audio."));
    }
    if audio && flag(&values, "closest-quality") && typed.contains("closest-quality") {
        return Err(Error::msg("--closest-quality is for video; omit it when using --audio."));
    }
    if flag(&values, "closest-quality") && quality == "best" && typed.contains("closest-quality") {
        return Err(Error::msg("--closest-quality requires a numeric --quality such as 1080p."));
    }
    let list_qualities = flag(&values, "list-qualities") || flag(&values, "list-qualitys");
    let single = positionals.len() == 1 && batch.is_none() && retry.is_none();
    if list_qualities {
        if !single {
            return Err(Error::msg("--list-qualities accepts exactly one URL."));
        }
        if flag(&values, "list-formats") || flag(&values, "list-sources") || flag(&values, "list-subs") {
            return Err(Error::msg("--list-qualities cannot be combined with other listing actions."));
        }
        if audio || flag(&values, "playlist") {
            return Err(Error::msg("--list-qualities requires a single video without --audio or --playlist."));
        }
    }
    let format = text(&values, "format");
    if flag(&values, "list-formats") && (audio || format.is_some()) {
        return Err(Error::msg("--list-formats cannot be combined with --audio or --format."));
    }
    if flag(&values, "list-formats") && positionals.len() > 1 {
        return Err(Error::msg("--list-formats accepts exactly one URL."));
    }
    if flag(&values, "list-subs") {
        if !single {
            return Err(Error::msg("--list-subs accepts exactly one URL."));
        }
        if flag(&values, "list-formats") || flag(&values, "list-sources") {
            return Err(Error::msg("--list-subs cannot be combined with other listing actions."));
        }
        if flag(&values, "playlist") {
            return Err(Error::msg("--list-subs requires a single video without --playlist."));
        }
    }
    if let Some(sub_format) = text(&values, "sub-format") {
        if !re!(r"(?i)^[a-z0-9]+(?:/[a-z0-9]+)*$").is_match(&sub_format) || sub_format.len() > 100 {
            return Err(Error::msg("--sub-format must be a format or preference list such as srt/vtt/best."));
        }
    }
    let source = text(&values, "source");
    if let Some(source) = &source {
        if !re!(r"^[1-9][0-9]*$").is_match(source) || source.parse::<u64>().map_or(true, |value| value > 9_007_199_254_740_991) {
            return Err(Error::msg("--source requires a positive source number."));
        }
    }
    if source.is_some() && flag(&values, "list-sources") && !typed.contains("list-sources") {
        set(&mut values, "list-sources", Some(Val::Bool(false)));
    }
    if flag(&values, "list-sources") && !typed.contains("list-sources") && !single {
        set(&mut values, "list-sources", Some(Val::Bool(false)));
    }
    if flag(&values, "list-sources") && !single {
        return Err(Error::msg("--list-sources accepts exactly one URL."));
    }
    if source.is_some() && (positionals.len() > 1 || batch.is_some()) {
        return Err(Error::msg("--source accepts exactly one URL."));
    }
    if source.is_some() && flag(&values, "list-sources") {
        return Err(Error::msg("--source and --list-sources are separate actions."));
    }
    let timeout = text(&values, "timeout");
    let timeout_ms = timeout.as_deref().map(parse_source_timeout).transpose()?;
    if (source.is_some() || flag(&values, "list-sources")) && !typed.contains("playlist") && !typed.contains("playlist-items") {
        set(&mut values, "playlist", Some(Val::Bool(false)));
        set(&mut values, "playlist-items", None);
    }
    if (source.is_some() || flag(&values, "list-sources")) && flag(&values, "playlist") {
        return Err(Error::msg("--source and --list-sources cannot be combined with --playlist."));
    }
    let fragments = parse_number(&text(&values, "concurrent-fragments").unwrap_or_else(|| "8".into()));
    crate::experimental::validate_fragments(fragments)?;
    let concurrency = parse_number(&text(&values, "playlist-concurrency").unwrap_or_default());
    if !(1.0..=4.0).contains(&concurrency) || concurrency.fract() != 0.0 {
        return Err(Error::msg("--playlist-concurrency must be a whole number between 1 and 4."));
    }
    let recode = flag(&values, "recode");
    let compatible = flag(&values, "compatible");
    if compatible
        && (audio || format.as_deref().is_some_and(|format| format != "mp4") || recode || flag(&values, "embed-subs") || flag(&values, "embed-thumbnail"))
    {
        return Err(Error::msg("--compatible requires video MP4 without --recode, --embed-subs or --embed-thumbnail."));
    }
    let format = if compatible { Some("mp4".to_string()) } else { format };
    if recode && (format.is_none() || audio) {
        return Err(Error::msg("--recode requires --format and video mode."));
    }
    let section = text(&values, "section");
    if section.as_ref().is_some_and(|section| !re!(r"^[*0-9]").is_match(js_trim(section))) {
        return Err(Error::msg("--section requires a range such as \"*10:00-12:00\" or \"10:00-12:00\"."));
    }
    let sponsor = text(&values, "sponsorblock-remove");
    if sponsor.as_ref().is_some_and(|value| !re!(r"(?i)^[a-z_,-]+$").is_match(js_trim(value))) {
        return Err(Error::msg("--sponsorblock-remove takes comma-separated category names, e.g. \"sponsor,selfpromo\"."));
    }
    let formats: &[&str] = if audio { &AUDIO_FORMATS } else { &VIDEO_FORMATS };
    if let Some(format) = &format {
        if !formats.contains(&format.as_str()) {
            let hint = if !audio && AUDIO_FORMATS.contains(&format.as_str()) { " Use --audio for audio formats." } else { "" };
            return Err(Error::msg(format!("Invalid {} format. Choose: {}.{hint}", if audio { "audio" } else { "video" }, formats.join(", "))));
        }
    }
    let concurrent_downloads = parse_number(&text(&values, "concurrent-downloads").unwrap_or_default());
    let urls: Vec<String> = positionals.iter().map(|url| validate_url(url)).collect::<Result<_>>()?;
    let mut options = Options {
        quality,
        output,
        rename,
        format,
        compatible,
        recode,
        concurrent_downloads: concurrent_downloads.max(0.0) as u32,
        filename_template: text(&values, "filename-template"),
        folder_template: text(&values, "folder-template"),
        adaptive_concurrency: flag(&values, "adaptive-concurrency"),
        check_space: flag(&values, "check-space"),
        timings: flag(&values, "timings"),
        color: flag(&values, "color"),
        playlist_concurrency: concurrency as u32,
        cookies: text(&values, "cookies").map(|file| validate_cookie_file(&file)).transpose()?,
        cookies_from_browser: text(&values, "cookies-from-browser").map(|spec| validate_browser_spec(&spec)).transpose()?,
        concurrent_fragments: fragments as u32,
        experimental_fragments: flag(&values, "experimental-fragments"),
        sub_langs: text(&values, "sub-langs"),
        sponsorblock_remove: sponsor.map(|value| js_trim(&value).to_string()),
        section: section.map(|value| js_trim(&value).to_string()),
        open: flag(&values, "open"),
        audio,
        resume: flag(&values, "resume"),
        incognito,
        neutral_filename: flag(&values, "neutral-filename"),
        playlist: flag(&values, "playlist"),
        subs: flag(&values, "subs"),
        auto_subs: flag(&values, "auto-subs"),
        sub_format: text(&values, "sub-format"),
        verify: flag(&values, "verify"),
        embed_subs: flag(&values, "embed-subs"),
        embed_metadata: flag(&values, "embed-metadata"),
        embed_thumbnail: flag(&values, "embed-thumbnail"),
        closest_quality: flag(&values, "closest-quality"),
        json: flag(&values, "json"),
        skip_existing: flag(&values, "skip-existing"),
        playlist_items: text(&values, "playlist-items"),
        source,
        timeout,
        deep_scan: flag(&values, "deep-scan"),
        list_sources: flag(&values, "list-sources"),
        auto_list_sources: flag(&values, "auto-list-sources"),
        list_qualities,
        list_formats: flag(&values, "list-formats"),
        list_subs: flag(&values, "list-subs"),
        dry_run: flag(&values, "dry-run"),
        batch_file: batch,
        retry_failed: retry,
        url: urls.first().cloned().unwrap_or_default(),
        urls,
        timeout_ms,
        profile: profile_name,
        ..Options::default()
    };
    if !(1..=4).contains(&options.concurrent_downloads) || concurrent_downloads.fract() != 0.0 {
        return Err(Error::msg("--concurrent-downloads must be a whole number between 1 and 4."));
    }
    if let Some(template) = &options.filename_template {
        validate_template(template, false)?;
    }
    if let Some(template) = &options.folder_template {
        validate_template(template, true)?;
    }
    if options.rename.is_some() && options.filename_template.is_some() && !options.neutral_filename {
        return Err(Error::msg("--rename and --filename-template cannot be combined."));
    }
    let disable_subs = !options.subs && typed.contains("subs");
    if options.sub_langs.is_some() && !disable_subs && !options.auto_subs {
        options.subs = true;
    }
    if disable_subs {
        options.embed_subs = false;
        if !options.auto_subs {
            options.sub_langs = None;
        }
    }
    Ok(options)
}

/// JavaScript `Number(text)` for option values; NaN for anything else.
fn parse_number(text: &str) -> f64 {
    let trimmed = js_trim(text);
    if trimmed.is_empty() {
        return 0.0;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// Help, version, release notes and the update commands report versions
/// themselves; anything else earns the daily update hint on success.
pub fn should_update_notice(args: &[String]) -> bool {
    if args.iter().any(|arg| ["-h", "--help", "-v", "--version"].contains(&arg.as_str())) {
        return false;
    }
    !args.first().is_some_and(|arg| ["update", "up", "upgrade", "check", "version", "help", "changes", "changelog"].contains(&arg.as_str()))
}

const SUBCOMMANDS: [&str; 16] = [
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

fn report<T>(stderr: &Out, result: Result<T>, success: impl FnOnce(T) -> i32) -> i32 {
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

fn run_main(mut args: Vec<String>) -> i32 {
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

/// Results of searching a page for media.
#[derive(Default)]
pub struct PageSource {
    pub media_url: Option<String>,
    pub source: Option<usize>,
    pub discovered: Option<Discovered>,
    pub needs_selection: bool,
}

type Prefetched = Mutex<HashMap<String, Metadata>>;

pub fn prefetch_key(options: &Options) -> String {
    format!("{}\n{}\n{}", options.url, options.media_url.as_deref().unwrap_or(""), options.playlist)
}

/// For a single page, fall back to media requests observed while its player
/// loads. A completed local transfer skips all network work.
pub fn resolve_page_source(
    options: &Options,
    cancel: &Cancel,
    stderr: &Out,
    reporter: &dyn Reporter,
    backend: &BackendResolver,
    prefetched: &Prefetched,
    interactive: bool,
) -> Result<PageSource> {
    let listing = options.dry_run || options.list_sources || options.list_formats || options.list_qualities || options.list_subs;
    if !options.incognito
        && !listing
        && downloader::read_ready_transfer(options, &crate::download_cache::root(), cancel, None, crate::state::now_ms())?.is_some()
    {
        return Ok(PageSource::default());
    }
    let backend: Backend = backend(cancel, reporter)?;
    let inspect =
        |request: &Options, scan: &Cancel| downloader::fetch_metadata(request, &backend, &downloader::run_backend, scan, &crate::progress::NullReporter);
    let mut original: Option<Error> = None;
    if options.source.is_none() && !options.list_sources {
        match inspect(options, cancel) {
            Ok(metadata) if metadata.has_entries() || !metadata.formats().is_empty() || metadata.url.is_some() => {
                prefetched.lock().unwrap().insert(prefetch_key(options), metadata);
                return Ok(PageSource::default());
            }
            Ok(_) => original = Some(Error::msg("No downloadable video formats found.")),
            Err(error) => original = Some(error),
        }
        let error = original.clone().unwrap();
        // A transient failure is retried with back-off by the download itself.
        if options.adaptive_concurrency && !listing && crate::execution::retryable(&error) {
            return Ok(PageSource::default());
        }
        if !should_offer_source_discovery(&error) {
            return Err(error);
        }
        if !options.auto_list_sources {
            return Err(Error::msg(format!("{} Use --list-sources to search this page, then --source <number> to select one.", error.message)));
        }
        stderr.write("veo: No direct video found; searching the page for a playable source…\n");
    }
    let candidate = |url: &str, scan: &Cancel| inspect(&Options { media_url: Some(url.to_string()), ..options.clone() }, scan);
    let discovered =
        match source_discovery::discover_sources(&options.url, cancel, options.deep_scan, options.timeout_ms, &candidate, &source_discovery::default_capture) {
            Ok(discovered) => discovered,
            Err(error) => {
                if cancel.is_cancelled() || error.is_cancelled() {
                    return Err(error);
                }
                if let Some(original) = &original {
                    return Err(Error::msg(format!("{} Source discovery also failed: {}", original.message, error.message)));
                }
                return Err(error);
            }
        };
    if options.list_sources {
        return Ok(PageSource { discovered: Some(discovered), ..PageSource::default() });
    }
    if discovered.timed_out {
        stderr.write(&format!("veo: Source search timed out after {}; showing verified sources found so far.\n", timeout_label(options)));
    }
    if discovered.sources.is_empty() {
        return Err(original.unwrap_or_else(|| Error::msg("No downloadable video sources were found on this page.")));
    }
    let mut index: usize = options.source.as_deref().and_then(|source| source.parse().ok()).unwrap_or(0);
    if index == 0 {
        if discovered.sources.len() == 1 {
            index = 1;
        } else if !interactive {
            return Ok(PageSource { discovered: Some(discovered), needs_selection: true, ..PageSource::default() });
        }
    }
    if index == 0 {
        for source in &discovered.sources {
            stderr.write(&format!("{}\n", format_source(source)));
        }
        let answer = terminal::ask("Source number (Enter to cancel): ", Std::Err)?;
        if !re!(r"^[1-9][0-9]*$").is_match(answer.trim()) {
            return Err(Error::msg("No source selected."));
        }
        index = answer.trim().parse().unwrap_or(0);
    }
    let selected = discovered
        .sources
        .get(index.wrapping_sub(1))
        .ok_or_else(|| Error::msg(format!("Source {index} does not exist; choose 1-{}.", discovered.sources.len())))?;
    Ok(PageSource { media_url: Some(selected.url.clone()), source: Some(index), ..PageSource::default() })
}

fn timeout_label(options: &Options) -> String {
    options.timeout.clone().unwrap_or_else(|| if options.deep_scan { "120s".into() } else { "45s".into() })
}

fn print_sources(stdout: &Out, url: &str, discovered: &Discovered, json: bool) {
    if json {
        stdout.line(sources_json(url, discovered).to_string());
    } else {
        for source in &discovered.sources {
            stdout.line(format_source(source));
        }
    }
}

/// Backends prepared during page resolution, reused by the download.
struct SharedBackend {
    prepared: Mutex<Option<Backend>>,
}

impl SharedBackend {
    fn resolve(&self, cancel: &Cancel, reporter: &dyn Reporter) -> Result<Backend> {
        let mut prepared = self.prepared.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Some(backend) = prepared.as_ref() {
            return Ok(backend.clone());
        }
        let backend = downloader::default_backend(cancel, reporter)?;
        *prepared = Some(backend.clone());
        Ok(backend)
    }
}

fn run_download(args: Vec<String>, stdout: &Out, stderr: &Out) -> i32 {
    let reporter = TerminalReporter::new();
    let root: Arc<dyn Reporter> = Arc::new(Root(reporter.clone()));
    let cancel = Cancel::global();
    let mut registration = None;
    let result = (|| -> Result<i32> {
        let loaded = load_config(None)?;
        for warning in &loaded.warnings {
            stderr.write(&format!("veo: {warning}\n"));
        }
        let mut args = args;
        if args.is_empty() && terminal::stdin_is_tty() && Std::Err.is_tty() {
            match crate::interactive::interactive_args(&loaded.config, cancel)? {
                Some(answers) => args = answers,
                None => return Ok(0),
            }
        }
        let mut options = parse_cli(&args, &loaded.config)?;
        if options.help {
            stdout.write(&help::build(options.help_all));
            return Ok(0);
        }
        if options.version {
            stdout.write(&format!("{CURRENT}\n"));
            return Ok(0);
        }
        if !options.incognito {
            registration = Some(crate::runs::register_run(cancel.clone(), &paths::cache_base())?);
        }
        let mut retry_items: Option<Vec<Options>> = None;
        if let Some(retry) = options.retry_failed.clone() {
            let items = jobs::retry_options(&retry, &paths::cache_base())?;
            // Revalidate stored options and apply only flags explicitly supplied now.
            let mut retry_args = Vec::new();
            let mut skip = false;
            for (index, arg) in args.iter().enumerate() {
                if skip {
                    skip = false;
                    continue;
                }
                if arg == "--retry-failed" {
                    skip = index + 1 < args.len();
                    continue;
                }
                if arg.starts_with("--retry-failed=") {
                    continue;
                }
                retry_args.push(arg.clone());
            }
            let override_profile = retry_args.iter().any(|arg| arg == "--profile" || arg.starts_with("--profile="));
            let mut parsed = Vec::new();
            for item in items {
                let url = item.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
                let mut config = item.clone();
                if override_profile {
                    if let Some(profiles) = loaded.config.get("profiles") {
                        config.insert("profiles".into(), profiles.clone());
                    }
                }
                let mut resumed = parse_cli(&[url].into_iter().chain(retry_args.iter().cloned()).collect::<Vec<_>>(), &config)?;
                if !override_profile {
                    resumed.profile = item.get("profile").and_then(Value::as_str).map(str::to_string);
                }
                parsed.push(resumed);
            }
            let urls = parsed.iter().map(|item| item.url.clone()).collect();
            options = Options { urls, ..parsed[0].clone() };
            retry_items = Some(parsed);
        }
        if let Some(batch) = options.batch_file.clone() {
            let content = String::from_utf8_lossy(&std::fs::read(&batch)?).into_owned();
            for line in content.trim_start_matches('\u{feff}').lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    options.urls.push(validate_url(line)?);
                }
            }
            options.url = options.urls.first().cloned().unwrap_or_default();
            if options.urls.is_empty() {
                return Err(Error::msg("The URL list is empty."));
            }
            if !options.neutral_filename && options.rename.as_ref().is_some_and(|rename| !rename.contains('*')) && options.urls.len() > 1 {
                return Err(Error::msg("--rename only applies to a single URL unless the name contains * for the original title."));
            }
            if options.list_formats && options.urls.len() > 1 {
                return Err(Error::msg("--list-formats accepts exactly one URL."));
            }
        }
        let requests: Vec<Options> = retry_items.clone().unwrap_or_else(|| vec![options.clone()]);
        crate::experimental::confirm_experimental_fragments(&requests)?;
        // The job file exists before the first download, so `veo runs <id>` can follow it.
        let listing = options.list_qualities || options.list_formats || options.list_sources || options.list_subs || options.dry_run;
        let job_file = if options.incognito || listing { None } else { Some(jobs::job_file_path(&paths::cache_base())) };
        if let Some(registration) = &registration {
            registration.describe(&crate::runs::RunDetails {
                urls: &options.urls,
                output: Some(paths::display(paths::resolve(&options.output))),
                audio: options.audio,
                quality: &options.quality,
                format: options.format.as_deref(),
                profile: options.profile.as_deref(),
                playlist: options.playlist,
                job: job_file.as_ref().map(paths::display),
            })?;
        }
        reporter.configure(options.color && !options.json);
        reporter.start(options.rename.as_deref().unwrap_or(""));
        if !listing {
            reporter.profile(options.profile.as_deref());
        }
        if let Some(warning) = cookie_file_warning(options.cookies.as_deref()) {
            stderr.write(&format!("veo: {warning}\n"));
        }
        let shared = SharedBackend { prepared: Mutex::new(None) };
        let backend_resolver = |cancel: &Cancel, reporter: &dyn Reporter| shared.resolve(cancel, reporter);
        let prefetched: Prefetched = Mutex::new(HashMap::new());
        let interactive = terminal::stdin_is_tty() && Std::Err.is_tty() && !options.json;
        if retry_items.is_none() && options.urls.len() == 1 && (!options.playlist || options.list_sources) {
            let page = resolve_page_source(&options, cancel, stderr, root.as_ref(), &backend_resolver, &prefetched, interactive)?;
            if let Some(discovered) = &page.discovered {
                print_sources(stdout, &options.url, discovered, options.json);
                if options.list_sources {
                    if discovered.timed_out {
                        stderr.write(&format!("veo: Source search timed out after {}; showing verified sources found so far.\n", timeout_label(&options)));
                    }
                    if discovered.sources.is_empty() {
                        stderr.write(if discovered.timed_out {
                            "veo: No sources were verified before the timeout.\n"
                        } else {
                            "veo: No downloadable sources were found on this page.\n"
                        });
                    }
                    return Ok(if !discovered.sources.is_empty() && !discovered.timed_out { 0 } else { 1 });
                }
                if page.needs_selection {
                    stderr.write("veo: Select a source with --source <number>.\n");
                    return Ok(1);
                }
            }
            if let Some(media) = page.media_url {
                options.media_url = Some(media);
                options.source = page.source.map(|source| source.to_string());
            }
        }
        if let Some(items) = retry_items.as_mut() {
            for item in items.iter_mut().filter(|item| item.source.is_some()) {
                let page = resolve_page_source(item, cancel, stderr, root.as_ref(), &backend_resolver, &prefetched, false)?;
                item.media_url = page.media_url;
            }
        }
        let tools = Tools { cancel, reporter: root.as_ref(), backend: &backend_resolver, runner: &downloader::run_backend };
        let cached = |options: &Options| prefetched.lock().unwrap().remove(&prefetch_key(options));
        if options.list_subs {
            let result = downloader::list_subtitles(&options, &tools, cached(&options))?;
            if options.json {
                stdout.line(result.to_string());
            } else {
                stdout.line(format!("Subtitles: {}", clean_text(result["title"].as_str().unwrap_or(&options.url))));
                for (label, key) in [("Manual", "manual"), ("Automatic", "automatic")] {
                    stdout.line(format!("{label}:"));
                    let entries = result[key].as_array().cloned().unwrap_or_default();
                    if entries.is_empty() {
                        stdout.line("  none");
                    }
                    for entry in entries {
                        let name = entry["name"].as_str().map(|name| format!(" ({})", clean_text(name))).unwrap_or_default();
                        let formats: Vec<&str> = entry["formats"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                        stdout.line(format!(
                            "  {}{name}: {}",
                            clean_text(entry["language"].as_str().unwrap_or("")),
                            if formats.is_empty() { "unknown".into() } else { formats.join(", ") }
                        ));
                    }
                }
            }
            return Ok(0);
        }
        if options.list_qualities {
            let result = downloader::list_qualities(&options, &tools, cached(&options))?;
            if options.json {
                stdout.line(result.to_string());
            } else {
                let qualities: Vec<&str> = result["qualities"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                stdout.line(format!("Available qualities: {}", if qualities.is_empty() { "resolution unknown".into() } else { qualities.join(", ") }));
                if let Some(first) = qualities.first() {
                    stdout.line(format!("Choose with --quality {first}"));
                }
            }
            return Ok(0);
        }
        if options.list_formats {
            stdout.write(&downloader::list_formats(&options, &tools)?);
            return Ok(0);
        }
        if options.dry_run {
            for request in retry_items.clone().unwrap_or_else(|| options.urls.iter().map(|url| options.with_url(url)).collect()) {
                let prefetched = cached(&request);
                let (plan, estimate) = downloader::plan_download(&request, &tools, prefetched)?;
                if options.json {
                    let mut line = Map::new();
                    line.insert("url".into(), Value::String(request.url.clone()));
                    line.insert("status".into(), Value::String("planned".into()));
                    for (key, value) in plan.clone() {
                        line.insert(key, value);
                    }
                    stdout.line(Value::Object(line).to_string());
                } else {
                    print_plan(stdout, &request.url, &plan, &estimate);
                }
            }
            return Ok(0);
        }
        if !options.json && Std::Out.is_tty() {
            reporter.enable_inline();
        }
        let live: Arc<dyn crate::output::LiveRegion> = reporter.clone();
        let job_stdout = stdout.clone().with_live(live.clone());
        let job_stderr = stderr.clone().with_live(live);
        let local_root = crate::download_cache::root();
        let download = |request: &Options,
                        item_reporter: Arc<dyn Reporter>,
                        adaptive: &Adaptive,
                        on_entry: &(dyn Fn(&downloader::EntryOutcome) -> Result<()> + Sync),
                        private: Option<&Path>| {
            let deps = Deps {
                cancel,
                reporter: item_reporter,
                backend: &backend_resolver,
                runner: &downloader::run_backend,
                compatibility_runner: &downloader::run_backend,
                local_root: private.map_or_else(|| local_root.clone(), Path::to_path_buf),
                skip_cache_cleanup: true,
                adaptive,
                wait: &real_wait,
                disk_checker: &downloader::default_disk_checker,
                verifier: &downloader::default_verifier,
                on_entry: Some(on_entry),
            };
            downloader::download_with(request, &deps, cached(request))
        };
        jobs::run_job(
            &options,
            JobContext {
                download: &download,
                reporter: root.clone(),
                stdout: job_stdout,
                stderr: job_stderr,
                cancel,
                open_file: &crate::open_file::open_file,
                items: retry_items,
                job_file,
                run_id: registration.as_ref().map(|registration| registration.id.clone()),
                stats: (!options.incognito).then(|| crate::stats::StatsRecorder::new(&paths::cache_base())),
                history: (!options.incognito).then(|| crate::history::HistoryRecorder::new(&paths::cache_base())),
                archive_root: paths::cache_base(),
            },
        )
    })();
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            let cancelled = cancel.is_cancelled() || error.is_cancelled();
            reporter.fail(cancelled);
            reporter.dispose();
            stderr.write(&format!("veo: {}\n", readable_error(&error)));
            if cancelled {
                130
            } else {
                1
            }
        }
    };
    reporter.dispose();
    if let Some(registration) = registration {
        registration.unregister();
    }
    code
}

fn print_plan(stdout: &Out, url: &str, plan: &Map<String, Value>, estimate: &Estimate) {
    stdout.line(format!("URL: {url}"));
    stdout.line(describe_estimate(estimate));
    if let Some(quality) = plan.get("quality").and_then(Value::as_str) {
        stdout.line(quality);
    }
    for entry in plan.get("entries").and_then(Value::as_array).into_iter().flatten() {
        let target = entry["path"].as_str().unwrap_or("");
        let rows = path_rows(stdout.tty, "Would save: ", target);
        stdout.line(
            rows.iter()
                .enumerate()
                .map(|(line, row)| format!("{}{row}", if line > 0 { " ".repeat(12) } else { "Would save: ".into() }))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
