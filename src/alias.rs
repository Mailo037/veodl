use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::json;

use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths;
use crate::utils::{clean_text, readable_error};

pub const BUILTIN_ALIASES: [&str; 2] = ["veo", "veodl"];
pub const ALIAS_MARKER: &str = "veo-alias";

pub const ALIAS_HELP: &str = "veo alias - manage extra command names for veo

Usage:
  veo alias                    List command names (same as: veo alias list)
  veo alias list [--json]      Show veo, veodl plus custom wrappers
  veo alias add <name> [--force] [--json]
                               Create <name> as a wrapper that calls veo
  veo alias remove <name> [--json]   Delete a wrapper (also: veo alias rm)
  veo alias --help             Show this help

Names use 2-31 lowercase letters, digits or hyphens and start with a letter,
e.g. veo-dl. The wrapper is created next to the veo command found on PATH and
simply forwards to veo, so it survives veo updates. Removing veodl only
removes that shortcut; veo update recreates the shipped names. veo itself can
never be removed. Use --bin-dir <path> to override the target directory.
";

pub fn validate_alias_name(input: &str) -> Result<String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(Error::msg("An alias name is required. Use: veo alias add <name>"));
    }
    if clean_text(raw) != raw {
        return Err(Error::msg(format!("Invalid alias name: {raw}. Use plain text without escapes.")));
    }
    if !re!(r"^[a-z][a-z0-9-]{1,30}$").is_match(raw) {
        return Err(Error::msg(format!("Invalid alias name \"{raw}\". Use 2-31 lowercase letters, digits or hyphens, starting with a letter (e.g. veo-dl).")));
    }
    Ok(raw.to_string())
}

pub fn alias_files(bin_dir: &Path, name: &str) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![bin_dir.join(name), bin_dir.join(format!("{name}.cmd")), bin_dir.join(format!("{name}.ps1"))]
    } else {
        vec![bin_dir.join(name)]
    }
}

fn shell_wrapper(name: &str) -> String {
    format!("#!/bin/sh\n# {ALIAS_MARKER}: {name}\nexec veo \"$@\"\n")
}

fn is_file(file: &Path) -> Result<bool> {
    match fs::metadata(file) {
        Ok(info) => Ok(info.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn managed(file: &Path) -> bool {
    fs::read(file).map(|bytes| String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).contains(ALIAS_MARKER)).unwrap_or(false)
}

pub fn resolve_bin_dir(bin_dir: Option<&str>) -> Result<PathBuf> {
    if let Some(bin_dir) = bin_dir {
        return Ok(paths::resolve(bin_dir));
    }
    let found = crate::backend::find(&["veo"])
        .ok_or_else(|| Error::msg("Could not locate the veo command on PATH. Install veo globally (npm install -g veodl) or pass --bin-dir <path>."))?;
    Ok(paths::dirname(found))
}

#[derive(Clone, Debug, Serialize)]
pub struct AliasStatus {
    pub name: String,
    pub builtin: bool,
    pub present: bool,
    pub managed: bool,
    pub paths: Vec<String>,
}

pub fn alias_status(name: &str, bin_dir: &Path) -> Result<AliasStatus> {
    let mut present = Vec::new();
    let mut is_managed = false;
    for file in alias_files(bin_dir, name) {
        if is_file(&file)? {
            if managed(&file) {
                is_managed = true;
            }
            present.push(paths::display(&file));
        }
    }
    Ok(AliasStatus { name: name.into(), builtin: BUILTIN_ALIASES.contains(&name), present: !present.is_empty(), managed: is_managed, paths: present })
}

fn base_name(file: &str) -> String {
    if cfg!(windows) {
        let lower = file.to_lowercase();
        if lower.ends_with(".cmd") || lower.ends_with(".ps1") {
            return file[..file.len() - 4].to_string();
        }
    }
    file.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasList {
    pub bin_dir: String,
    pub aliases: Vec<AliasStatus>,
}

pub fn list_aliases(bin_dir: Option<&str>) -> Result<AliasList> {
    let resolved = resolve_bin_dir(bin_dir)?;
    let mut entries: Vec<AliasStatus> = BUILTIN_ALIASES.iter().map(|name| alias_status(name, &resolved)).collect::<Result<_>>()?;
    let listing = fs::read_dir(&resolved)
        .map_err(|error| Error::msg(format!("Cannot list the command directory {}: {}", resolved.display(), readable_error(&error.into()))))?;
    for entry in listing.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if !managed(&path) {
            continue;
        }
        let base = base_name(&entry.file_name().to_string_lossy());
        if validate_alias_name(&base).is_err() {
            continue;
        }
        if let Some(existing) = entries.iter_mut().find(|item| item.name == base) {
            existing.managed = true;
            continue;
        }
        entries.push(AliasStatus { name: base, builtin: false, present: true, managed: true, paths: vec![paths::display(&path)] });
    }
    entries.sort_by(|a, b| crate::utils::locale_cmp(&a.name, &b.name));
    Ok(AliasList { bin_dir: paths::display(&resolved), aliases: entries })
}

pub fn add_alias(name: &str, bin_dir: Option<&str>, force: bool) -> Result<(String, PathBuf, Vec<PathBuf>)> {
    let valid = validate_alias_name(name)?;
    if valid == "veo" {
        return Err(Error::msg("veo is already the main command."));
    }
    let resolved = resolve_bin_dir(bin_dir)?;
    fs::create_dir_all(&resolved)?;
    let files = alias_files(&resolved, &valid);
    let present: Vec<&PathBuf> = files.iter().filter(|file| is_file(file).unwrap_or(false)).collect();
    if !present.is_empty() && !force {
        return Err(Error::msg(format!(
            "{valid} already exists ({}). Use --force to replace it.",
            present.iter().map(paths::basename).collect::<Vec<_>>().join(", ")
        )));
    }
    for file in present {
        let _ = fs::remove_file(file);
    }
    fs::write(&files[0], shell_wrapper(&valid))?;
    // Non-POSIX filesystems may ignore modes; the shebang still works.
    let _ = crate::state::set_mode(&files[0], 0o755);
    if cfg!(windows) {
        fs::write(&files[1], format!("@echo off\r\nREM {ALIAS_MARKER}: {valid}\r\nveo %*\r\n"))?;
        fs::write(&files[2], format!("# {ALIAS_MARKER}: {valid}\nveo @args\n"))?;
    }
    Ok((valid, resolved, files))
}

pub fn remove_alias(name: &str, bin_dir: Option<&str>) -> Result<(String, PathBuf, Vec<String>, bool)> {
    let valid = validate_alias_name(name)?;
    if valid == "veo" {
        return Err(Error::msg("Refusing to remove veo itself. Remove veodl or a custom wrapper instead."));
    }
    let resolved = resolve_bin_dir(bin_dir)?;
    let status = alias_status(&valid, &resolved)?;
    if !status.present {
        return Err(Error::msg(format!("{valid} is not installed in {}.", resolved.display())));
    }
    for file in &status.paths {
        crate::fsutil::remove_file(Path::new(file))?;
    }
    Ok((valid, resolved, status.paths, status.builtin))
}

pub struct AliasArgs {
    pub bin_dir: Option<String>,
    pub force: bool,
    pub json: bool,
    pub rest: Vec<String>,
}

fn parse_alias_args(args: &[String]) -> Result<AliasArgs> {
    let mut parsed = AliasArgs { bin_dir: None, force: false, json: false, rest: Vec::new() };
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--force" {
            parsed.force = true;
        } else if token == "--json" {
            parsed.json = true;
        } else if token == "--bin-dir" {
            index += 1;
            parsed.bin_dir = Some(
                args.get(index)
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .ok_or_else(|| Error::msg("--bin-dir requires a directory. Run veo alias --help for usage."))?,
            );
        } else if let Some(value) = token.strip_prefix("--bin-dir=") {
            if value.is_empty() {
                return Err(Error::msg("--bin-dir requires a directory. Run veo alias --help for usage."));
            }
            parsed.bin_dir = Some(value.into());
        } else {
            parsed.rest.push(token.clone());
        }
        index += 1;
    }
    Ok(parsed)
}

pub fn alias_main(args: &[String], stdout: &Out) -> Result<i32> {
    let args = crate::output::strip_color_flags(args);
    let parsed = parse_alias_args(&args)?;
    let (command, name, extra) = (parsed.rest.first().map(String::as_str), parsed.rest.get(1), parsed.rest.len() > 2);
    match command {
        None | Some("list") => {
            if name.is_some() || extra {
                return Err(Error::msg("Usage: veo alias list [--json] (or veo alias --help)"));
            }
            let result = list_aliases(parsed.bin_dir.as_deref())?;
            if parsed.json {
                stdout.line(serde_json::to_string(&result)?);
                return Ok(0);
            }
            stdout.line(format!("Command directory: {}", result.bin_dir));
            for entry in &result.aliases {
                let kind = if entry.builtin { "shipped" } else { "custom" };
                let state = if !entry.present {
                    "missing"
                } else if entry.managed {
                    "wrapper -> veo"
                } else {
                    "installed"
                };
                stdout.line(format!("  {} ({kind}, {state})", entry.name));
            }
            let missing: Vec<&str> = result.aliases.iter().filter(|entry| !entry.present).map(|entry| entry.name.as_str()).collect();
            if !missing.is_empty() {
                stdout.line(format!("Missing: {} (veo update recreates shipped names).", missing.join(", ")));
            }
            Ok(0)
        }
        Some("-h" | "--help") => {
            stdout.write(ALIAS_HELP);
            Ok(0)
        }
        Some("add") => {
            let Some(name) = name.filter(|_| !extra) else { return Err(Error::msg("Usage: veo alias add <name> [--force] (or veo alias --help)")) };
            let (name, bin_dir, files) = add_alias(name, parsed.bin_dir.as_deref(), parsed.force)?;
            if parsed.json {
                stdout.line(serde_json::to_string(&json!({ "status": "added", "name": name, "binDir": paths::display(&bin_dir), "paths": files.iter().map(paths::display).collect::<Vec<_>>() }))?);
                return Ok(0);
            }
            stdout.line(format!("Alias installed: {name} -> veo ({})", bin_dir.display()));
            stdout.line(format!("Run \"{name} --help\" to use it. Remove it with: veo alias remove {name}"));
            Ok(0)
        }
        Some("remove" | "rm" | "uninstall") => {
            let Some(name) = name.filter(|_| !extra) else { return Err(Error::msg("Usage: veo alias remove <name> (or veo alias --help)")) };
            print_removed(remove_alias(name, parsed.bin_dir.as_deref())?, parsed.json, stdout)
        }
        Some(other) => {
            Err(Error::msg(format!("Unknown alias command: {}. Use: veo alias list | veo alias add <name> | veo alias remove <name>", clean_text(other))))
        }
    }
}

pub fn print_removed((name, bin_dir, removed, builtin): (String, PathBuf, Vec<String>, bool), json: bool, stdout: &Out) -> Result<i32> {
    if json {
        stdout.line(serde_json::to_string(
            &json!({ "status": "removed", "name": name, "binDir": paths::display(&bin_dir), "removed": removed, "builtin": builtin }),
        )?);
        return Ok(0);
    }
    stdout.line(format!("Alias removed: {name} ({} file(s) in {})", removed.len(), bin_dir.display()));
    if builtin {
        stdout.line("Note: veo update recreates the shipped veodl name.");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_names() {
        assert!(validate_alias_name("veo-dl").is_ok());
        assert!(validate_alias_name("V").is_err());
        assert!(validate_alias_name("1abc").is_err());
    }

    #[test]
    fn adds_lists_and_removes_wrappers() {
        let root = crate::fsutil::temp_dir(&std::env::temp_dir(), "veo-alias-").unwrap();
        let bin = paths::display(&root);
        add_alias("vdl", Some(&bin), false).unwrap();
        assert!(add_alias("vdl", Some(&bin), false).is_err());
        let listed = list_aliases(Some(&bin)).unwrap();
        assert!(listed.aliases.iter().any(|alias| alias.name == "vdl" && alias.managed));
        assert!(listed.aliases.iter().any(|alias| alias.name == "veo" && !alias.present));
        let (_, _, removed, builtin) = remove_alias("vdl", Some(&bin)).unwrap();
        assert_eq!(removed.len(), if cfg!(windows) { 3 } else { 1 });
        assert!(!builtin);
        let _ = fs::remove_dir_all(root);
    }
}
