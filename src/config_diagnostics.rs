use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::config::{apply_profile, effective_config, parse_config_text, CONFIG_KEYS, RESERVED_PROFILE_NAMES};
use crate::config_template::strip_config_comments;
use crate::error::{Error, Result};
use crate::utils::{AUDIO_FORMATS, QUALITIES, VIDEO_FORMATS};

/// Source span of one JSON value (byte offsets into the original text).
#[derive(Clone, Debug)]
pub struct Node {
    pub path: Vec<Value>,
    pub start: usize,
    pub end: usize,
    pub key_start: Option<usize>,
    pub key_end: Option<usize>,
}

pub fn path_key(path: &[Value]) -> String {
    serde_json::to_string(path).unwrap_or_default()
}

/// Parse a config after comment stripping and record the span of every value,
/// including escaped property names and settings repeated across profiles.
pub fn config_source(text: &str) -> Result<(Value, HashMap<String, Node>)> {
    let clean = strip_config_comments(text)?;
    let data: Value = serde_json::from_str(&clean)?;
    let tokens: Vec<(usize, &str)> = crate::re!(r#""(?:\\.|[^"\\])*"|-?[0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?|true|false|null|[{}\[\]:,]"#)
        .find_iter(&clean)
        .map(|found| (found.start(), found.as_str()))
        .collect();
    let bom = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let mut nodes = HashMap::new();
    let mut index = 0;
    fn visit(
        tokens: &[(usize, &str)],
        index: &mut usize,
        path: Vec<Value>,
        key: Option<(usize, &str)>,
        bom: usize,
        nodes: &mut HashMap<String, Node>,
    ) -> Option<()> {
        let first = *tokens.get(*index)?;
        *index += 1;
        if first.1 == "{" {
            while tokens.get(*index)?.1 != "}" {
                let key_token = tokens[*index];
                *index += 2; // key and colon
                let name: String = serde_json::from_str(key_token.1).ok()?;
                let mut child = path.clone();
                child.push(Value::String(name));
                visit(tokens, index, child, Some(key_token), bom, nodes)?;
                if tokens.get(*index)?.1 == "," {
                    *index += 1;
                }
            }
            *index += 1;
        } else if first.1 == "[" {
            let mut item = 0u64;
            while tokens.get(*index)?.1 != "]" {
                let mut child = path.clone();
                child.push(Value::from(item));
                item += 1;
                visit(tokens, index, child, None, bom, nodes)?;
                if tokens.get(*index)?.1 == "," {
                    *index += 1;
                }
            }
            *index += 1;
        }
        let last = tokens[*index - 1];
        nodes.insert(
            path_key(&path),
            Node {
                path: path.clone(),
                start: first.0 + bom,
                end: last.0 + last.1.len() + bom,
                key_start: key.map(|key| key.0 + bom),
                key_end: key.map(|key| key.0 + key.1.len() + bom),
            },
        );
        Some(())
    }
    visit(&tokens, &mut index, Vec::new(), None, bom, &mut nodes).ok_or_else(|| Error::msg("Invalid JSON structure."))?;
    Ok((data, nodes))
}

fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, left) in a.iter().enumerate() {
        let mut next = vec![i + 1];
        for (j, right) in b.iter().enumerate() {
            next.push((next[j] + 1).min(row[j + 1] + 1).min(row[j] + usize::from(left != right)));
        }
        row = next;
    }
    row[b.len()]
}

fn keys() -> Vec<String> {
    CONFIG_KEYS.iter().map(|(key, _)| key.to_string()).collect()
}

fn suggestion(key: &str, choices: &[String]) -> Option<String> {
    let lower: Vec<char> = key.to_lowercase().chars().collect();
    let mut ranked: Vec<(usize, &String)> = choices.iter().map(|value| (distance(&lower, &value.to_lowercase().chars().collect::<Vec<_>>()), value)).collect();
    ranked.sort_by_key(|(distance, _)| *distance);
    ranked.first().filter(|(distance, _)| *distance <= 2).map(|(_, value)| value.to_string())
}

/// A problem located in the config text (offsets in bytes; line/column 1-based).
#[derive(Clone, Debug)]
pub struct Issue {
    pub message: String,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

fn located(text: &str, node: Option<&Node>, message: String, property: bool, _choices: Vec<Value>) -> Issue {
    let start = node.and_then(|node| if property { node.key_start } else { Some(node.start) }).unwrap_or(0).min(text.len());
    let end = node.and_then(|node| if property { node.key_end } else { Some(node.end) }).unwrap_or(start + 1);
    let before = &text[..start];
    Issue {
        message,
        start: Some(start),
        end: Some(end),
        line: Some(before.matches('\n').count() + 1),
        column: Some(before.rfind('\n').map_or(before.chars().count(), |index| before[index + 1..].chars().count()) + 1),
    }
}

pub fn value_choices(key: &str, config: &Map<String, Value>) -> Vec<Value> {
    let strings = |values: &[&str]| values.iter().map(|value| Value::String(value.to_string())).collect();
    match key {
        _ if CONFIG_KEYS.iter().any(|(name, kind)| *name == key && *kind == "boolean") => vec![true.into(), false.into()],
        "quality" => strings(&QUALITIES),
        "format" => strings(if config.get("audio") == Some(&Value::Bool(true)) { &AUDIO_FORMATS[..] } else { &VIDEO_FORMATS[..] }),
        "timeout" => strings(&["30s", "45s", "1m", "2m", "5m"]),
        "concurrentDownloads" | "playlistConcurrency" => (1..=4).map(Value::from).collect(),
        "concurrentFragments" => (1..=64).map(Value::from).collect(),
        _ => Vec::new(),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn join_choices(choices: &[Value]) -> String {
    choices
        .iter()
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first semantic problem of a syntactically valid config, located in the text.
pub fn semantic_issue(text: &str) -> Result<Option<Issue>> {
    let (data, nodes) = config_source(text)?;
    let node = |path: &[Value]| nodes.get(&path_key(path));
    let Value::Object(root) = &data else {
        return Ok(Some(located(text, node(&[]), "The config must contain a JSON object.".into(), false, Vec::new())));
    };
    let check = |settings: &Map<String, Value>, path: &[Value]| -> Option<Issue> {
        for (key, value) in settings {
            if path.is_empty() && ["$schema", "profiles", "activeProfile"].contains(&key.as_str()) {
                continue;
            }
            let mut target_path = path.to_vec();
            target_path.push(Value::String(key.clone()));
            let target = node(&target_path);
            let Some((_, kind)) = CONFIG_KEYS.iter().find(|(name, _)| name == key) else {
                let mut choices = keys();
                if path.is_empty() {
                    choices.extend(["profiles".to_string(), "$schema".to_string()]);
                }
                let proposed = suggestion(key, &choices);
                let message = format!("Unknown setting \"{key}\".{}", proposed.as_ref().map(|value| format!(" Did you mean \"{value}\"?")).unwrap_or_default());
                return Some(located(text, target, message, true, proposed.into_iter().map(Value::String).collect()));
            };
            if type_name(value) != *kind {
                let choices = value_choices(key, settings);
                let listed = if choices.is_empty() { String::new() } else { format!(" Choose: {}.", join_choices(&choices)) };
                return Some(located(text, target, format!("\"{key}\" must be a {kind}.{listed}"), false, choices));
            }
        }
        None
    };
    if let Some(issue) = check(root, &[]) {
        return Ok(Some(issue));
    }
    if let Some(profiles) = root.get("profiles") {
        let Value::Object(profiles) = profiles else {
            return Ok(Some(located(text, node(&[Value::String("profiles".into())]), "Profiles must be an object.".into(), false, Vec::new())));
        };
        for (name, profile) in profiles {
            let path = [Value::String("profiles".into()), Value::String(name.clone())];
            if RESERVED_PROFILE_NAMES.contains(&name.as_str()) {
                return Ok(Some(located(
                    text,
                    node(&path),
                    format!("Profile name \"{name}\" is reserved for veo profile {name}. Rename this profile."),
                    true,
                    Vec::new(),
                )));
            }
            let Value::Object(profile) = profile else {
                return Ok(Some(located(text, node(&path), format!("Profile \"{name}\" must be an object."), false, Vec::new())));
            };
            if let Some(issue) = check(profile, &path) {
                return Ok(Some(issue));
            }
        }
    }
    if let Some(active) = root.get("activeProfile") {
        let path = [Value::String("activeProfile".into())];
        let Some(active) = active.as_str().filter(|active| !active.is_empty()) else {
            return Ok(Some(located(text, node(&path), "\"activeProfile\" must be a non-empty profile name.".into(), false, Vec::new())));
        };
        let names: Vec<String> = root.get("profiles").and_then(Value::as_object).map(|profiles| profiles.keys().cloned().collect()).unwrap_or_default();
        if !names.iter().any(|name| name == active) {
            let listed = if names.is_empty() { "none".to_string() } else { names.join(", ") };
            return Ok(Some(located(
                text,
                node(&path),
                format!("Unknown active profile \"{active}\". Choose: {listed}."),
                false,
                names.into_iter().map(Value::String).collect(),
            )));
        }
    }
    let config = parse_config_text(text, "config")?.config;
    let profiles: Vec<Option<String>> = std::iter::once(None)
        .chain(config.get("profiles").and_then(Value::as_object).into_iter().flat_map(|profiles| profiles.keys().cloned().map(Some)))
        .collect();
    for profile in profiles {
        let Err(error) = effective_config(&config, profile.as_deref()) else { continue };
        let message = error.message;
        let mut key = if message.contains("Invalid quality") {
            Some("quality".to_string())
        } else if crate::re!(r"Invalid (?:audio|video) format").is_match(&message) {
            Some("format".into())
        } else if message.contains("output directory") {
            Some("output".into())
        } else if message.contains("custom filename") {
            Some("rename".into())
        } else {
            crate::re!(r"--([a-z-]+)").captures(&message).map(|captures| {
                let mut camel = String::new();
                let mut upper = false;
                for c in captures[1].chars() {
                    if c == '-' {
                        upper = true;
                    } else if upper {
                        camel.extend(c.to_uppercase());
                        upper = false;
                    } else {
                        camel.push(c);
                    }
                }
                camel
            })
        };
        let selected = apply_profile(&config, profile.as_deref())?;
        // Template errors do not name the CLI option. Locate the failing template.
        if key.is_none() {
            for candidate in ["filenameTemplate", "folderTemplate"] {
                if let Some(Value::String(template)) = selected.get(candidate) {
                    if crate::naming::validate_template(template, candidate == "folderTemplate").is_err() {
                        key = Some(candidate.into());
                        break;
                    }
                }
            }
        }
        let key = key.unwrap_or_default();
        let name = profile
            .clone()
            .or_else(|| config.get("activeProfile").and_then(Value::as_str).map(str::to_string))
            .or_else(|| config.get("profiles").and_then(|profiles| profiles.get("default")).map(|_| "default".to_string()));
        let in_profile =
            name.as_ref().is_some_and(|name| config.get("profiles").and_then(|profiles| profiles.get(name)).and_then(|profile| profile.get(&key)).is_some());
        let target = if in_profile {
            node(&[Value::String("profiles".into()), Value::String(name.clone().unwrap_or_default()), Value::String(key.clone())])
        } else {
            node(&[Value::String(key.clone())])
        };
        let value = if key == "quality" || key == "format" {
            format!("\"{key}\": unsupported value {}. ", selected.get(&key).map_or("undefined".into(), Value::to_string))
        } else {
            String::new()
        };
        let prefix = name.map(|name| format!("Profile \"{name}\": ")).unwrap_or_default();
        return Ok(Some(located(text, target.or_else(|| node(&[])), format!("{prefix}{value}{message}"), false, value_choices(&key, &selected))));
    }
    Ok(None)
}

/// Replacement choices for the property or value under the cursor (byte offset).
pub struct Completion {
    pub start: usize,
    pub end: usize,
    pub choices: Vec<Value>,
}

pub fn editor_completions(text: &str, cursor: usize) -> Option<Completion> {
    let (data, nodes) = config_source(text).ok()?;
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    let mut ordered: Vec<&Node> = nodes.values().collect();
    ordered.sort_by_key(|node| node.start);
    for item in ordered {
        let path = &item.path;
        if !(path.len() == 1 || (path.len() == 3 && path[0] == "profiles")) {
            continue;
        }
        let key = path.last()?.as_str()?.to_string();
        let property = item.key_start.is_some_and(|start| cursor >= start) && item.key_end.is_some_and(|end| cursor <= end);
        if !property && !(cursor >= item.start && cursor <= item.end) {
            continue;
        }
        let choices: Vec<Value> = if property {
            let mut names = keys();
            if path.len() == 1 {
                names.extend(["profiles", "activeProfile", "$schema"].map(str::to_string));
            }
            names.into_iter().map(Value::String).collect()
        } else if key == "activeProfile" {
            root.get("profiles").and_then(Value::as_object).map(|profiles| profiles.keys().cloned().map(Value::String).collect()).unwrap_or_default()
        } else {
            let profile = (path.len() == 3).then(|| path[1].as_str().unwrap_or_default().to_string());
            let selected = apply_profile(root, profile.as_deref()).unwrap_or_default();
            value_choices(&key, &selected)
        };
        if choices.is_empty() {
            return None;
        }
        let proposed =
            if property { suggestion(&key, &choices.iter().filter_map(|value| value.as_str().map(str::to_string)).collect::<Vec<_>>()) } else { None };
        let choices = match proposed {
            Some(proposed) => {
                std::iter::once(Value::String(proposed.clone())).chain(choices.into_iter().filter(|value| value.as_str() != Some(proposed.as_str()))).collect()
            }
            None => choices,
        };
        let (start, end) = if property { (item.key_start?, item.key_end?) } else { (item.start, item.end) };
        return Some(Completion { start, end, choices });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_unknown_settings_with_suggestions() {
        let text = "{\n  \"qualty\": \"720p\"\n}";
        let issue = semantic_issue(text).unwrap().unwrap();
        assert!(issue.message.contains("Did you mean \"quality\"?"), "{}", issue.message);
        assert_eq!((issue.line, issue.column), (Some(2), Some(3)));
    }

    #[test]
    fn reports_invalid_profile_values() {
        let text = "{ \"profiles\": { \"x\": { \"quality\": \"999\" } } }";
        let issue = semantic_issue(text).unwrap().unwrap();
        assert!(issue.message.starts_with("Profile \"x\": \"quality\": unsupported value \"999\"."), "{}", issue.message);
        assert!(semantic_issue("{ \"quality\": \"720p\" }").unwrap().is_none());
    }

    #[test]
    fn offers_value_completions() {
        let text = "{ \"audio\": true }";
        let completion = editor_completions(text, text.find("true").unwrap()).unwrap();
        assert_eq!(completion.choices, vec![Value::Bool(true), Value::Bool(false)]);
    }
}
