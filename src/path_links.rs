use std::fs;
use std::path::Path;

use crate::paths;

fn env_is(name: &str, value: &str) -> bool {
    std::env::var(name).is_ok_and(|current| current == value)
}

pub fn supports_path_links(tty: bool) -> bool {
    tty && !crate::terminal::dumb()
        && (std::env::var_os("WT_SESSION").is_some()
            || env_is("TERM_PROGRAM", "iTerm.app")
            || env_is("TERM_PROGRAM", "WezTerm")
            || env_is("TERM_PROGRAM", "vscode")
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var("VTE_VERSION").ok().and_then(|value| value.parse::<u32>().ok()).is_some_and(|version| version >= 5000))
}

fn safe_label(label: &str) -> String {
    label.chars().map(|c| if matches!(c, '\u{00}'..='\u{1f}' | '\u{7f}'..='\u{9f}') { ' ' } else { c }).collect()
}

/// File links target their containing folder so clicking does not play the media.
pub fn folder_link(tty: bool, target: &Path, label: &str) -> String {
    let label = safe_label(label);
    if !supports_path_links(tty) {
        return label;
    }
    let absolute = paths::resolve(target);
    let Ok(info) = fs::metadata(&absolute) else { return label };
    let folder = if info.is_dir() { absolute } else { paths::dirname(&absolute) };
    match url::Url::from_directory_path(&folder) {
        Ok(url) => format!("\x1b]8;;{url}\x1b\\{label}\x1b]8;;\x1b\\"),
        Err(()) => label,
    }
}

pub fn folder_link_path(tty: bool, target: &Path) -> String {
    folder_link(tty, target, &paths::display(target))
}

fn path_start(text: &str, index: usize) -> bool {
    let rest = &text[index..];
    re!(r"^[A-Za-z]:[\\/]").is_match(rest) || re!(r"^\\\\[^\\\s]+\\").is_match(rest) || (rest.starts_with('/') && !rest[1..].starts_with('/'))
}

/// Recognize absolute paths in application prose before display truncation.
/// The longest existing candidate wins, so spaces and parentheses survive.
pub fn link_output_paths(tty: bool, original: &str, displayed: &str) -> String {
    if !supports_path_links(tty)
        || original.contains('\x1b')
        || re!(r#"^\s*(?:["{\[]|Retry:|Usage:|Manual command:|Run with|Stop:|Cleanup:)"#).is_match(original)
    {
        return displayed.to_string();
    }
    let mut starts = Vec::new();
    let mut previous: Option<char> = None;
    for (index, c) in original.char_indices() {
        let boundary = previous.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '"' | '\''));
        if boundary && path_start(original, index) {
            starts.push(index);
        }
        previous = Some(c);
    }
    let mut displayed = displayed.to_string();
    for start in starts.into_iter().rev() {
        let rest = &original[start..];
        let mut ends = vec![rest.len()];
        for (index, c) in rest.char_indices().rev() {
            if index > 0 && (c.is_whitespace() || matches!(c, ')' | '"' | '\'' | ',' | ';' | ':' | '.')) {
                ends.push(index);
            }
        }
        let Some(target) = ends.into_iter().map(|end| &rest[..end]).find(|candidate| fs::metadata(candidate).is_ok()) else { continue };
        let Some(visible) = displayed.get(start..) else { continue };
        if let Some(after) = visible.strip_prefix(target) {
            let link = folder_link(tty, Path::new(target), target);
            displayed = format!("{}{}{}", &displayed[..start], link, after);
        } else if let Some(prefix) = visible.strip_suffix('…') {
            if target.starts_with(prefix) {
                let link = folder_link(tty, Path::new(target), visible);
                displayed = format!("{}{}", &displayed[..start], link);
            }
        }
    }
    displayed
}
