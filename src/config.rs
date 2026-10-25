use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::config_errors::config_syntax_error;
use crate::config_template::{config_guide, strip_config_comments, CONFIG_TEMPLATE};
use crate::error::{Error, Result};
use crate::output::Out;
use crate::path_links::folder_link_path;
use crate::paths;
use crate::terminal::{self, Std};

/// Keys accepted in the config file, with their JSON type. Every value is a
/// default only: an explicit command-line flag always wins.
pub const CONFIG_KEYS: [(&str, &str); 46] = [
    ("stagingDir", "string"),
    ("fastCut", "boolean"),
    ("nativeHttp", "boolean"),
    ("httpConnections", "number"),
    ("concurrentDownloads", "number"),
    ("adaptiveConcurrency", "boolean"),
    ("filenameTemplate", "string"),
    ("folderTemplate", "string"),
    ("checkSpace", "boolean"),
    ("timings", "boolean"),
    ("color", "boolean"),
    ("output", "string"),
    ("quality", "string"),
    ("format", "string"),
    ("compatible", "boolean"),
    ("recode", "boolean"),
    ("playlistConcurrency", "number"),
    ("rename", "string"),
    ("audio", "boolean"),
    ("open", "boolean"),
    ("resume", "boolean"),
    ("incognito", "boolean"),
    ("neutralFilename", "boolean"),
    ("closestQuality", "boolean"),
    ("cookies", "string"),
    ("cookiesFromBrowser", "string"),
    ("playlist", "boolean"),
    ("concurrentFragments", "number"),
    ("experimentalFragments", "boolean"),
    ("subs", "boolean"),
    ("autoSubs", "boolean"),
    ("subLangs", "string"),
    ("subFormat", "string"),
    ("verify", "boolean"),
    ("embedSubs", "boolean"),
    ("embedMetadata", "boolean"),
    ("embedThumbnail", "boolean"),
    ("sponsorblockRemove", "string"),
    ("section", "string"),
    ("json", "boolean"),
    ("skipExisting", "boolean"),
    ("playlistItems", "string"),
    ("deepScan", "boolean"),
    ("timeout", "string"),
    ("listSources", "boolean"),
    ("autoListSources", "boolean"),
];

pub const RESERVED_PROFILE_NAMES: [&str; 2] = ["list", "reset"];

fn config_key(key: &str) -> Option<&'static str> {
    CONFIG_KEYS.iter().find(|(name, _)| *name == key).map(|(_, kind)| *kind)
}

pub fn config_file() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("VEO_CONFIG") {
        let text = value.to_string_lossy();
        if !text.trim().is_empty() {
            if text.contains('\0') {
                return Err(Error::msg("VEO_CONFIG must be a filesystem path."));
            }
            return Ok(paths::resolve(text.trim()));
        }
    }
    Ok(paths::config_base().join("config.json"))
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Object(_) => "object",
    }
}

#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub file: PathBuf,
    pub exists: bool,
    pub config: Map<String, Value>,
    pub warnings: Vec<String>,
}

/// Read optional user defaults. A missing file is not an error; a malformed one
/// is reported loudly, because silently ignoring a typo would be worse.
pub fn load_config(file: Option<&Path>) -> Result<Loaded> {
    let target = match file {
        Some(file) => file.to_path_buf(),
        None => config_file()?,
    };
    let text = match fs::read(&target) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Loaded { file: target, exists: false, ..Loaded::default() }),
        Err(error) => return Err(Error::msg(format!("Cannot read the veo config file {}: {error}", target.display()))),
    };
    let mut loaded = parse_config_text(&text, &paths::display(&target))?;
    loaded.file = target;
    Ok(loaded)
}

pub fn parse_config_text(text: &str, target: &str) -> Result<Loaded> {
    let data: Value = match strip_config_comments(text) {
        Ok(clean) => serde_json::from_str(&clean).map_err(|_| config_syntax_error(&clean, target, None))?,
        Err(error) => {
            let bare = text.strip_prefix('\u{feff}').unwrap_or(text);
            return Err(config_syntax_error(bare, target, Some((error.position.unwrap_or(0), error.message))));
        }
    };
    let Value::Object(data) = data else {
        return Err(Error::msg(format!("The veo config file must contain a JSON object: {target}")));
    };
    let mut warnings = Vec::new();
    let mut config = Map::new();
    for (key, value) in data {
        if key == "$schema" {
            continue;
        }
        if key == "activeProfile" {
            if !value.as_str().is_some_and(|name| !name.is_empty()) {
                return Err(Error::msg(format!("Config key \"activeProfile\" must be a non-empty string ({target}).")));
            }
            config.insert(key, value);
            continue;
        }
        if key == "profiles" {
            let Value::Object(profiles) = value else { return Err(Error::msg("Config profiles must be an object.")) };
            let mut checked_profiles = Map::new();
            for (name, profile) in profiles {
                if RESERVED_PROFILE_NAMES.contains(&name.as_str()) {
                    return Err(Error::msg(format!("Profile name \"{name}\" is reserved for veo profile {name}. Rename it in {target}.")));
                }
                let Value::Object(profile) = profile else { return Err(Error::msg(format!("Profile \"{name}\" must be an object."))) };
                let mut checked = Map::new();
                for (setting, item) in profile {
                    let expected = config_key(&setting).ok_or_else(|| Error::msg(format!("Unknown setting \"{setting}\" in profile \"{name}\".")))?;
                    if type_of(&item) != expected {
                        return Err(Error::msg(format!("Profile \"{name}\": \"{setting}\" must be a {expected}.")));
                    }
                    checked.insert(setting, item);
                }
                checked_profiles.insert(name, Value::Object(checked));
            }
            config.insert(key, Value::Object(checked_profiles));
            continue;
        }
        let Some(expected) = config_key(&key) else {
            warnings.push(format!("Unknown config key \"{key}\" in {target} was ignored."));
            continue;
        };
        if type_of(&value) != expected {
            return Err(Error::msg(format!("Config key \"{key}\" must be a {expected}, not a {} ({target}).", type_of(&value))));
        }
        config.insert(key, value);
    }
    if let Some(active) = config.get("activeProfile").and_then(Value::as_str) {
        if !profile_names(&config).iter().any(|name| name == active) {
            return Err(Error::msg(format!("Unknown active profile \"{active}\" in {target}. Available: {}.", available(&config))));
        }
    }
    Ok(Loaded { file: PathBuf::from(target), exists: true, config, warnings })
}

pub fn profile_names(config: &Map<String, Value>) -> Vec<String> {
    config.get("profiles").and_then(Value::as_object).map(|profiles| profiles.keys().cloned().collect()).unwrap_or_default()
}

fn available(config: &Map<String, Value>) -> String {
    let names = profile_names(config);
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

pub fn selected_profile_name(config: &Map<String, Value>, name: Option<&str>) -> Option<String> {
    name.map(str::to_string)
        .or_else(|| config.get("activeProfile").and_then(Value::as_str).map(str::to_string))
        .or_else(|| profile_names(config).into_iter().find(|name| name == "default"))
}

/// Global defaults merged with the selected profile.
pub fn apply_profile(config: &Map<String, Value>, name: Option<&str>) -> Result<Map<String, Value>> {
    let mut defaults: Map<String, Value> =
        config.iter().filter(|(key, _)| *key != "profiles" && *key != "activeProfile").map(|(key, value)| (key.clone(), value.clone())).collect();
    let Some(name) = selected_profile_name(config, name) else { return Ok(defaults) };
    let profile = config.get("profiles").and_then(|profiles| profiles.get(&name)).and_then(Value::as_object);
    let Some(profile) = profile else {
        return Err(Error::msg(format!("Unknown profile \"{name}\". Available: {}. Use veo config edit.", available(config))));
    };
    for (key, value) in profile {
        defaults.insert(key.clone(), value.clone());
    }
    Ok(defaults)
}

/// The resolved settings of a profile, without credentials.
pub fn effective_config(config: &Map<String, Value>, profile: Option<&str>) -> Result<Map<String, Value>> {
    let mut selected = apply_profile(config, profile)?;
    // Inspection is offline and never prints credential file paths or browser profiles.
    let cookies = selected.remove("cookies");
    let browser = selected.remove("cookiesFromBrowser");
    let parsed = crate::cli::parse_cli(&["https://example.invalid/config-check".to_string()], &selected)?;
    let mut result = Map::new();
    for (key, _) in &CONFIG_KEYS {
        if let Some(value) = parsed.get(key) {
            result.insert((*key).into(), value);
        }
    }
    if let Some(Value::String(output)) = result.get("output") {
        let resolved = paths::display(paths::resolve(output));
        result.insert("output".into(), Value::String(resolved));
    }
    if cookies.is_some() {
        result.insert("cookies".into(), Value::String("[configured]".into()));
    }
    if browser.is_some() {
        result.insert("cookiesFromBrowser".into(), Value::String("[configured]".into()));
    }
    Ok(result)
}

pub fn profile_main(args: &[String], file: &Path, output: &Out) -> Result<i32> {
    if args.len() > 1 || args.first().is_some_and(|arg| arg.starts_with('-')) {
        return Err(Error::msg("Usage: veo profile [list|reset|NAME]"));
    }
    let loaded = load_config(Some(file))?;
    let Some(argument) = args.first() else {
        output.line(format!("Active profile: {}", selected_profile_name(&loaded.config, None).unwrap_or_else(|| "global (no profile)".into())));
        output.line(format!("Available: {}", available(&loaded.config)));
        return Ok(0);
    };
    if argument == "list" {
        let names = profile_names(&loaded.config);
        if names.is_empty() {
            output.line("No profiles configured. Use veo config edit.");
        } else {
            let active = selected_profile_name(&loaded.config, None);
            let mut lines = vec!["Profiles:".to_string()];
            if active.is_none() {
                lines.push("* global (no profile)".into());
            }
            for name in names {
                let current = active.as_deref() == Some(name.as_str());
                lines.push(format!("{}{name}{}", if current { "* " } else { "  " }, if current { " (active)" } else { "" }));
            }
            output.line(lines.join("\n"));
        }
        return Ok(0);
    }
    let reset = argument == "reset";
    if !reset {
        apply_profile(&loaded.config, Some(argument))?;
        effective_config(&loaded.config, Some(argument))?;
    }
    if !loaded.exists {
        output.line("Active profile: global (no profile)");
        return Ok(0);
    }
    let original = String::from_utf8_lossy(&fs::read(file)?).into_owned();
    // Token locations come from the comment-aware parser, preserving hand-written notes.
    let (_, nodes) = crate::config_diagnostics::config_source(&original)?;
    let property = nodes.get(&crate::config_diagnostics::path_key(&[Value::String("activeProfile".into())])).cloned();
    let bom = if original.starts_with('\u{feff}') { "\u{feff}".len() } else { 0 };
    let clean = format!("{}{}", " ".repeat(bom), strip_config_comments(&original)?);
    let updated = if reset {
        match &property {
            None => original.clone(),
            Some(property) => {
                let (mut start, mut end) = (property.key_start.unwrap_or(property.start), property.end);
                let bytes = clean.as_bytes();
                while bytes.get(end).is_some_and(u8::is_ascii_whitespace) {
                    end += 1;
                }
                if bytes.get(end) == Some(&b',') {
                    end += 1;
                } else {
                    let mut before = start;
                    while before > 0 && bytes[before - 1].is_ascii_whitespace() {
                        before -= 1;
                    }
                    if before > 0 && bytes[before - 1] == b',' {
                        start = before - 1;
                    }
                    end = property.end;
                }
                format!("{}{}", &original[..start], &original[end..])
            }
        }
    } else if let Some(property) = &property {
        format!("{}{}{}", &original[..property.start], serde_json::to_string(argument)?, &original[property.end..])
    } else {
        let root = nodes.get(&crate::config_diagnostics::path_key(&[])).cloned().ok_or_else(|| Error::msg("Invalid config structure."))?;
        let inner = &clean[root.start + 1..root.end - 1];
        let first = inner.find(|c: char| !c.is_whitespace()).map(|offset| root.start + 1 + offset);
        let newline = if original.contains("\r\n") { "\r\n" } else { "\n" };
        let entry = format!("\"activeProfile\": {}", serde_json::to_string(argument)?);
        match first {
            None => format!("{}{newline}  {entry}{newline}{}", &original[..root.end - 1], &original[root.end - 1..]),
            Some(position) => {
                let line_start = original[..position].rfind('\n').map_or(0, |index| index + 1);
                let indent = &original[line_start..position];
                let indent = if indent.chars().all(|c| c == ' ' || c == '\t') { indent } else { "" };
                format!("{}{entry},{newline}{indent}{}", &original[..position], &original[position..])
            }
        }
    };
    let parsed = parse_config_text(&updated, &paths::display(file))?;
    if updated != original {
        if String::from_utf8_lossy(&fs::read(file)?) != original {
            return Err(Error::msg("Config changed while switching profiles. Try again."));
        }
        fs::write(file, &updated)?;
    }
    let active = if reset { selected_profile_name(&parsed.config, None).unwrap_or_else(|| "global (no profile)".into()) } else { argument.clone() };
    output.line(format!("Active profile: {active}"));
    Ok(0)
}

pub fn prepare_config_edit(file: &Path) -> Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    match crate::state::create_private(file) {
        Ok(mut handle) => std::io::Write::write_all(&mut handle, CONFIG_TEMPLATE.as_bytes())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let original = String::from_utf8_lossy(&fs::read(file)?).into_owned();
            if original.trim_start_matches('\u{feff}').trim().is_empty() {
                fs::write(file, CONFIG_TEMPLATE)?;
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn named(args: &[String]) -> Result<Option<String>> {
    let mut profile = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--profile" {
            index += 1;
            profile = Some(args.get(index).cloned().ok_or_else(|| Error::msg("Option '--profile <value>' argument missing"))?);
        } else if let Some(value) = arg.strip_prefix("--profile=") {
            profile = Some(value.to_string());
        } else if arg.starts_with('-') {
            return Err(Error::msg(format!("Unknown option '{arg}'")));
        } else {
            return Err(Error::msg("Usage: veo config check|show [--profile NAME]"));
        }
        index += 1;
    }
    Ok(profile)
}

pub fn config_main(args: &[String]) -> Result<i32> {
    let editor_color = !args.iter().any(|arg| arg == "--no-color") && std::env::var_os("NO_COLOR").is_none();
    let mut stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if matches!(args.first().map(String::as_str), Some("show" | "guide")) {
        stdout = Out::stdout(false);
    }
    let file = config_file()?;
    match args.first().map(String::as_str) {
        Some("reset") => {
            if args.len() != 1 {
                return Err(Error::msg("Usage: veo config reset"));
            }
            return crate::config_reset::reset_config(&file, &stdout);
        }
        Some(command @ ("check" | "show")) => {
            let profile = named(&args[1..])?;
            let loaded = load_config(Some(&file))?;
            if !loaded.warnings.is_empty() {
                return Err(Error::msg(loaded.warnings.join("\n")));
            }
            if command == "show" {
                let effective = effective_config(&loaded.config, profile.as_deref())?;
                stdout.line(serde_json::to_string_pretty(&effective)?);
            } else {
                let names: Vec<Option<String>> = match profile {
                    Some(name) => vec![Some(name)],
                    None => std::iter::once(None).chain(profile_names(&loaded.config).into_iter().map(Some)).collect(),
                };
                for name in &names {
                    effective_config(&loaded.config, name.as_deref())
                        .map_err(|error| Error::msg(format!("Profile {}: {}", name.as_deref().unwrap_or("default/global"), error.message)))?;
                }
                stdout.line(format!("Config OK: {} ({} effective configurations checked).", file.display(), names.len()));
            }
            return Ok(0);
        }
        _ => {}
    }
    let mode = if args.first().is_some_and(|arg| arg == "edit") {
        if args.len() > 2 || args.get(1).is_some_and(|arg| arg != "--external" && arg != "--terminal") {
            return Err(Error::msg("Usage: veo config edit [--external|--terminal]"));
        }
        let mode = args.get(1).map(|arg| arg[2..].to_string()).or_else(|| std::env::var("VEO_CONFIG_EDITOR").ok()).unwrap_or_else(|| "auto".into());
        if !["auto", "external", "terminal"].contains(&mode.as_str()) {
            return Err(Error::msg("VEO_CONFIG_EDITOR must be auto, external or terminal."));
        }
        mode
    } else if args.len() != 1 || !["path", "profiles", "guide"].contains(&args[0].as_str()) {
        return Err(Error::msg("Usage: veo config edit|path|profiles|guide|check|show|reset"));
    } else {
        match args[0].as_str() {
            "path" => stdout.line(folder_link_path(stdout.tty, &file)),
            "guide" => stdout.write(config_guide()),
            _ => {
                let loaded = load_config(None)?;
                let names = profile_names(&loaded.config);
                stdout.line(if names.is_empty() { "No profiles configured. Use veo config edit.".into() } else { names.join("\n") });
            }
        }
        return Ok(0);
    };
    prepare_config_edit(&file)?;
    let visual = std::env::var_os("VISUAL").filter(|value| !value.is_empty());
    let editor = std::env::var_os("EDITOR").filter(|value| !value.is_empty());
    if mode == "terminal" || (mode == "auto" && visual.is_none() && editor.is_none() && terminal::stdin_is_tty() && Std::Out.is_tty()) {
        crate::config_editor::edit_config(&file, editor_color)?;
        return Ok(0);
    }
    // Treat the editor as an executable path, never as shell code.
    let program = visual.or(editor).unwrap_or_else(|| if cfg!(windows) { "notepad.exe".into() } else { "vi".into() });
    let status = std::process::Command::new(&program)
        .arg(&file)
        .status()
        .map_err(|_| Error::msg(format!("Could not launch editor. Set EDITOR to an executable path or edit {}.", file.display())))?;
    if !status.success() {
        return Err(Error::msg(format!("Editor exited with {}. Config: {}", status.code().map_or("null".into(), |code| code.to_string()), file.display())));
    }
    load_config(None)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profiles_and_warns_about_unknown_keys() {
        let loaded = parse_config_text(
            "{\n// x\n\"quality\": \"720p\", \"mystery\": 1, \"profiles\": { \"music\": { \"audio\": true } }, \"activeProfile\": \"music\" }",
            "c",
        )
        .unwrap();
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(selected_profile_name(&loaded.config, None).as_deref(), Some("music"));
        let applied = apply_profile(&loaded.config, None).unwrap();
        assert_eq!(applied["audio"], true);
        assert_eq!(applied["quality"], "720p");
        assert!(apply_profile(&loaded.config, Some("nope")).is_err());
    }

    #[test]
    fn rejects_invalid_types_and_names() {
        assert!(parse_config_text("{\"quality\": 720}", "c").unwrap_err().message.contains("must be a string, not a number"));
        assert!(parse_config_text("{\"profiles\": {\"list\": {}}}", "c").unwrap_err().message.contains("reserved"));
        assert!(parse_config_text("{\"activeProfile\": \"x\"}", "c").unwrap_err().message.contains("Unknown active profile"));
        assert!(parse_config_text("{,}", "c").unwrap_err().message.contains("not valid JSON"));
    }

    #[test]
    fn switches_profiles_in_place() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-config-").unwrap();
        let file = root.join("config.json");
        fs::write(&file, "{\n  // keep me\n  \"profiles\": { \"music\": { \"audio\": true } }\n}\n").unwrap();
        let out = Out::new(Std::Out, true);
        profile_main(&["music".into()], &file, &out).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("// keep me\n  \"activeProfile\": \"music\",\n  \"profiles\""), "{text}");
        profile_main(&["reset".into()], &file, &out).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("activeProfile") && text.contains("// keep me"), "{text}");
        let _ = fs::remove_dir_all(root);
    }
}
