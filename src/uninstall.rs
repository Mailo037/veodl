use serde_json::json;

use crate::alias::{list_aliases, print_removed, remove_alias, validate_alias_name};
use crate::config::config_file;
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths;
use crate::updater::{npm_command, PACKAGE};
use crate::utils::readable_error;

pub fn uninstall_help() -> String {
    format!(
        "veo uninstall - remove veo completely or one command alias

Usage:
  veo uninstall [-p|--prefix <name>] [--json]
                               Remove one alias (e.g. veo uninstall -p veodl)
  veo uninstall [--yes] [--keep-cache] [--keep-config] [--keep-aliases] [--json]
                               Remove everything: aliases, package, cache, config
  veo uninstall --help         Show this help

With -p/--prefix, only that alias is deleted; veo itself stays installed.
Without -p, veo prints what would be removed. Add --yes to delete everything:
custom wrappers created with veo alias add, the {PACKAGE} package itself
(commands veo, veodl) with npm, veo's cache directory (downloads, jobs,
backend tools, history, statistics) and the config file. Use --keep-cache,
--keep-config or --keep-aliases to preserve one part. The config file is
deleted by default; its path is shown before and after.
"
    )
}

#[derive(Default)]
struct Args {
    prefix: Option<String>,
    yes: bool,
    keep_cache: bool,
    keep_config: bool,
    keep_aliases: bool,
    json: bool,
    bin_dir: Option<String>,
    rest: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args> {
    let mut parsed = Args::default();
    let missing_prefix = || Error::msg("--prefix requires an alias name. Use: veo uninstall -p veodl");
    let mut index = 0;
    while index < args.len() {
        let token = args[index].as_str();
        match token {
            "-p" | "--prefix" => {
                index += 1;
                parsed.prefix = Some(args.get(index).filter(|value| !value.is_empty()).cloned().ok_or_else(missing_prefix)?);
            }
            "--yes" => parsed.yes = true,
            // Deprecated: full cleanup is now the default.
            "--purge" => {}
            "--keep-cache" => parsed.keep_cache = true,
            "--keep-config" => parsed.keep_config = true,
            "--keep-aliases" => parsed.keep_aliases = true,
            "--json" => parsed.json = true,
            "--bin-dir" => {
                index += 1;
                parsed.bin_dir = Some(args.get(index).filter(|value| !value.is_empty()).cloned().ok_or_else(|| Error::msg("--bin-dir requires a directory."))?);
            }
            _ => {
                if let Some(value) = token.strip_prefix("-p=").or_else(|| token.strip_prefix("--prefix=")) {
                    if value.is_empty() {
                        return Err(missing_prefix());
                    }
                    parsed.prefix = Some(value.into());
                } else if let Some(value) = token.strip_prefix("--bin-dir=") {
                    if value.is_empty() {
                        return Err(Error::msg("--bin-dir requires a directory."));
                    }
                    parsed.bin_dir = Some(value.into());
                } else {
                    parsed.rest.push(token.into());
                }
            }
        }
        index += 1;
    }
    Ok(parsed)
}

pub fn uninstall_main(args: &[String], stdout: &Out, stderr: &Out) -> Result<i32> {
    let args = crate::output::strip_color_flags(args);
    let parsed = parse(&args)?;
    if parsed.rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        stdout.write(&uninstall_help());
        return Ok(0);
    }
    if !parsed.rest.is_empty() {
        return Err(Error::msg(format!("Unknown option for veo uninstall: {}. Use: veo uninstall --help", parsed.rest.join(" "))));
    }
    if let Some(prefix) = &parsed.prefix {
        if parsed.keep_cache || parsed.keep_config || parsed.keep_aliases {
            return Err(Error::msg("--keep-* cannot be combined with --prefix. Remove one alias or uninstall veo itself."));
        }
        let valid = validate_alias_name(prefix)?;
        return print_removed(remove_alias(&valid, parsed.bin_dir.as_deref())?, parsed.json, stdout);
    }
    let cache = paths::cache_base();
    let config = config_file()?;
    let manual = format!("npm uninstall -g {PACKAGE}");
    // Custom wrappers first so the plan names them; npm removes veo/veodl itself.
    let (custom, alias_dir) = match list_aliases(parsed.bin_dir.as_deref()) {
        Ok(listed) => (
            listed.aliases.iter().filter(|entry| entry.present && entry.managed && !entry.builtin).map(|entry| entry.name.clone()).collect::<Vec<_>>(),
            Some(listed.bin_dir),
        ),
        Err(_) => (Vec::new(), parsed.bin_dir.as_deref().map(|dir| paths::display(paths::resolve(dir)))),
    };
    if !parsed.yes {
        if parsed.json {
            stdout.line(serde_json::to_string(&json!({ "status": "planned", "package": PACKAGE, "manual": manual, "aliases": custom, "binDir": alias_dir, "cache": paths::display(&cache), "config": paths::display(&config) }))?);
            return Ok(0);
        }
        stdout.line(format!("This removes everything: the {PACKAGE} package with npm (commands veo, veodl)."));
        if !parsed.keep_aliases {
            stdout.line(if custom.is_empty() {
                format!("Custom wrappers: none found{}.", alias_dir.as_ref().map(|dir| format!(" ({dir})")).unwrap_or_default())
            } else {
                format!("Custom wrappers to delete: {} ({})", custom.join(", "), alias_dir.clone().unwrap_or_default())
            });
        }
        if !parsed.keep_cache {
            stdout.line(format!("Cache to delete: {}", cache.display()));
        }
        if !parsed.keep_config {
            stdout.line(format!("Config to delete: {}", config.display()));
        }
        let kept: Vec<&str> = [(parsed.keep_aliases, "aliases"), (parsed.keep_cache, "cache"), (parsed.keep_config, "config")]
            .iter()
            .filter(|(keep, _)| *keep)
            .map(|(_, name)| *name)
            .collect();
        if !kept.is_empty() {
            stdout.line(format!("Kept: {}.", kept.join(", ")));
        }
        let flags = format!(
            "{}{}{}",
            if parsed.keep_aliases { " --keep-aliases" } else { "" },
            if parsed.keep_cache { " --keep-cache" } else { "" },
            if parsed.keep_config { " --keep-config" } else { "" }
        );
        stdout.line(format!("Run with --yes to proceed: veo uninstall --yes{flags}"));
        stdout.line(format!("Manual command: {manual}"));
        return Ok(0);
    }
    stdout.line(format!("Uninstalling {PACKAGE} with npm…"));
    let released = crate::updater::release_running_binary();
    let status = npm_command(&["uninstall", "-g", PACKAGE]).status();
    if !status.as_ref().is_ok_and(|status| status.success()) {
        crate::updater::restore_binary(released);
    }
    match status {
        Err(error) => {
            let reason = if error.kind() == std::io::ErrorKind::NotFound {
                "npm was not found. Install Node.js/npm, or uninstall manually.".to_string()
            } else {
                format!("Could not run npm: {}", readable_error(&error.into()))
            };
            stderr.write(&format!("veo: {reason}\nManual command: {manual}\n"));
            return Ok(1);
        }
        Ok(status) if !status.success() => {
            stderr.write(&format!(
                "veo: npm exited with code {}. Uninstall manually with: {manual}\n",
                status.code().map_or("null".into(), |code| code.to_string())
            ));
            return Ok(1);
        }
        Ok(_) => {}
    }
    let mut removed = Vec::new();
    if !parsed.keep_aliases {
        for name in &custom {
            if let Err(error) = remove_alias(name, alias_dir.as_deref()) {
                stderr.write(&format!("veo: removed the package but could not delete alias {name}: {}\n", readable_error(&error)));
                return Ok(1);
            }
            removed.push(name.clone());
        }
    }
    let mut cache_deleted = false;
    if !parsed.keep_cache {
        if let Err(error) = crate::fsutil::remove_all(&cache) {
            stderr.write(&format!("veo: removed the package but could not delete the cache {}: {}\n", cache.display(), readable_error(&error)));
            return Ok(1);
        }
        cache_deleted = true;
    }
    let mut config_deleted = false;
    if !parsed.keep_config {
        if let Err(error) = crate::fsutil::remove_file(&config) {
            stderr.write(&format!("veo: removed the package but could not delete the config {}: {}\n", config.display(), readable_error(&error)));
            return Ok(1);
        }
        config_deleted = true;
    }
    if parsed.json {
        stdout.line(serde_json::to_string(&json!({ "status": "uninstalled", "package": PACKAGE, "aliases": removed, "binDir": alias_dir, "cacheDeleted": cache_deleted, "cache": paths::display(&cache), "configDeleted": config_deleted, "config": paths::display(&config) }))?);
        return Ok(0);
    }
    let mut text = format!("veo uninstalled ({PACKAGE}).");
    if !parsed.keep_aliases {
        text.push_str(&if removed.is_empty() { " No custom wrappers found.".to_string() } else { format!(" Aliases deleted: {}.", removed.join(", ")) });
    }
    text.push_str(&if cache_deleted { format!(" Cache deleted: {}.", cache.display()) } else { format!(" Cache kept: {}.", cache.display()) });
    text.push_str(&if config_deleted { format!(" Config deleted: {}.", config.display()) } else { format!(" Config kept: {}.", config.display()) });
    stdout.line(text);
    Ok(0)
}
