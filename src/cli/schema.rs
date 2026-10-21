//! Option schema, strict argument parsing and config defaults.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::paths;
use crate::suggest::suggest;

pub(super) const STRING_OPTIONS: [(&str, Option<char>); 22] = [
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

pub(super) const BOOLEAN_OPTIONS: [(&str, Option<char>); 33] = [
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

pub(super) fn string_options() -> impl Iterator<Item = &'static (&'static str, Option<char>)> {
    STRING_OPTIONS.iter().filter(|(name, _)| !name.is_empty())
}

pub(super) fn boolean_options() -> impl Iterator<Item = &'static (&'static str, Option<char>)> {
    BOOLEAN_OPTIONS.iter().filter(|(name, _)| !name.is_empty())
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    String,
    Boolean,
}

pub(super) fn kind_of(name: &str) -> Option<Kind> {
    if string_options().any(|(option, _)| *option == name) {
        Some(Kind::String)
    } else if boolean_options().any(|(option, _)| *option == name) {
        Some(Kind::Boolean)
    } else {
        None
    }
}

pub(super) fn long_for_short(short: char) -> Option<&'static str> {
    string_options().chain(boolean_options()).find(|(_, letter)| *letter == Some(short)).map(|(name, _)| *name)
}

pub(super) fn short_and_long(name: &str) -> String {
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
pub(super) enum Val {
    Str(String),
    Bool(bool),
}

pub(super) struct Parsed {
    pub(super) values: HashMap<String, Val>,
    pub(super) positionals: Vec<String>,
    pub(super) typed: HashSet<String>,
}

pub(super) fn unknown(option: &str) -> Error {
    let suggestion = suggest(option, &option_spellings());
    Error::msg(format!(
        "Unknown option \"{option}\".{}",
        suggestion.map_or(" Run veo --help to see available options.".into(), |suggestion| format!(" Did you mean \"{suggestion}\"?"))
    ))
}

/// Strict argument parsing with `--no-` negation, like Node's util.parseArgs.
pub(super) fn parse_args(args: &[String], defaults: &HashMap<&'static str, Val>) -> Result<Parsed> {
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

pub(super) fn config_text(config: &Map<String, Value>, key: &str) -> Option<String> {
    match config.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub(super) fn config_flag(config: &Map<String, Value>, key: &str, fallback: bool) -> bool {
    match config.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => fallback,
        Some(other) => !matches!(other, Value::String(text) if text.is_empty()) && other.as_f64() != Some(0.0),
    }
}

/// Config defaults, so an explicit flag always wins but a stored preference does not.
pub(super) fn option_defaults(config: &Map<String, Value>) -> HashMap<&'static str, Val> {
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

pub(super) const COMMAND_FLAGS: [(&str, &[&str]); 17] = [
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

pub(super) fn command_names() -> Vec<String> {
    COMMAND_FLAGS.iter().map(|(name, _)| name.to_string()).chain(["changelog", "retry", "subs", "help", "version"].map(str::to_string)).collect()
}

pub(super) fn mistyped_command_option(args: &[String]) -> Option<String> {
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
