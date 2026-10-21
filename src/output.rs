use std::sync::Arc;

use crate::error::{Error, Result};
use crate::path_links::link_output_paths;
use crate::terminal::{self, columns, style, terminal_title_width, Std};
use crate::utils::{clean_text, js_trim};

const HELP_HEADINGS: [&str; 7] = [
    "Download:",
    "Subtitles and metadata:",
    "Playlists and batches:",
    "Naming and privacy:",
    "Inspect before downloading:",
    "Network and performance:",
    "Output and scripting:",
];
const COUNTER_LABELS: [&str; 7] = ["Tracking since:", "Videos saved:", "Audio saved:", "Total failures:", "Skipped:", "Cancelled:", "Download time:"];

/// `--profile` and color flags shared by every command.
pub struct DisplayOptions {
    pub profile: Option<String>,
    pub color: Option<bool>,
    pub remaining: Vec<String>,
}

pub fn output_options(args: &[String]) -> Result<DisplayOptions> {
    let mut profile = None;
    let mut color = None;
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            remaining.extend_from_slice(&args[index..]);
            break;
        }
        if arg == "--profile" || arg.starts_with("--profile=") {
            let value = if arg == "--profile" {
                index += 1;
                args.get(index).cloned()
            } else {
                Some(arg["--profile=".len()..].to_string())
            };
            match value {
                Some(value) if !value.is_empty() && !value.starts_with("--") => profile = Some(value),
                _ => return Err(Error::msg("--profile requires a profile name.")),
            }
        } else {
            if arg == "--color" {
                color = Some(true);
            }
            if arg == "--no-color" {
                color = Some(false);
            }
            remaining.push(arg.clone());
        }
        index += 1;
    }
    Ok(DisplayOptions { profile, color, remaining })
}

fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find('\n') {
        let (line, separator) = if index > 0 && rest.as_bytes()[index - 1] == b'\r' { (&rest[..index - 1], "\r\n") } else { (&rest[..index], "\n") };
        parts.push((line, separator));
        rest = &rest[index + 1..];
    }
    parts.push((rest, ""));
    parts
}

/// Style application prose by line: headings, results, warnings and errors get
/// a role color; long display values are fitted to the terminal width.
pub fn format_output(text: &str, tty: bool, enabled: bool) -> String {
    let mut result = String::with_capacity(text.len() + 32);
    for (line, separator) in split_lines(text) {
        result.push_str(&format_line(line, tty, enabled));
        result.push_str(separator);
    }
    result
}

fn format_line(line: &str, tty: bool, enabled: bool) -> String {
    if js_trim(line).is_empty() || line.contains('\x1b') {
        return line.to_string();
    }
    let value = js_trim(line).to_string();
    let original = line;
    let mut line = line.to_string();
    // Limit display values, keeping help, JSON/config text and runnable commands intact.
    if tty {
        let indent: String = original.chars().take_while(|c| c.is_whitespace()).collect();
        let available = columns().saturating_sub(indent.chars().count()).max(2);
        let width = available.saturating_sub(1).max(1);
        if re!(r"^[0-9]+\. ").is_match(&value) {
            line = terminal_title_width(Some(width), &value).split('\n').map(|part| format!("{indent}{part}")).collect::<Vec<_>>().join("\n");
        } else if re!(r"^(?:URL[s]?:|Saved:|Would save:|File:|Missing:|Output:|Job:|Error:|Profile:|Media:|Format:|Streams:|Source:|Title:|Command directory:|Config OK:|veo:|\[?(?:OK|FAIL)\]?\s|[0-9a-z]{6}\s+[0-9]+\s|https?://)").is_match(&value)
            || re!(r"^\s{6,}\S").is_match(original)
        {
            line = format!("{indent}{}", terminal::fit(&clean_text(&value), width));
        }
    }
    // Report rows such as `  ok    Node.js  v22`: only the status word carries
    // color, so values stay readable; details of passing rows recede.
    if let Some(report) = re!(r"^(\s*)(ok|warn|fail)(\s{2,})(\S.*)$").captures(&line) {
        let (indent, level, gap, rest) = (&report[1], &report[2], &report[3], &report[4]);
        let role = match level {
            "ok" => "success",
            "warn" => "warn",
            _ => "error",
        };
        let leading = original.len() - original.trim_start().len();
        let detail_original = original.get(leading + level.len() + gap.len()..).unwrap_or(rest);
        let detail = link_output_paths(tty, detail_original, rest);
        let detail = if level == "ok" { style(tty, &detail, "muted", enabled) } else { detail };
        return format!("{indent}{}{gap}{detail}", style(tty, level, role, enabled));
    }
    // `Label:   value` rows (veo stats): muted labels, values in the default color.
    if let Some(counter) = re!(r"^([A-Z][A-Za-z ]+:)(\s+)(\S.*)$").captures(&line) {
        let (label, gap, value) = (&counter[1], &counter[2], &counter[3]);
        if COUNTER_LABELS.contains(&label) {
            let failing = label == "Total failures:" && re!(r"^[1-9]").is_match(value);
            let value = if failing { style(tty, value, "error", enabled) } else { value.to_string() };
            return format!("{}{gap}{value}", style(tty, label, "muted", enabled));
        }
    }
    let mut role = "muted";
    if re!(r"^(?:veo (?:stats|history|changes|changelog|runs|stop|doctor|flush|update|backend|alias|uninstall)\b|veo [0-9][^ ]* doctor|ID\s+PID\s+STATE|Usage:|Options:|Commands:|Examples:|[0-9]+\. )").is_match(&value) {
        role = "title";
    }
    let indented = line.starts_with(char::is_whitespace);
    // A bare version line introduces one release in `veo changes` range output.
    if !indented && re!(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?(?:\s+\(installed\))?$").is_match(&value) {
        role = "title";
    }
    if !indented && HELP_HEADINGS.contains(&value.as_str()) {
        role = "title";
    }
    if re!(r"(?i)^(?:\[?OK\]?\s|Saved:|Flushed:|Stopped\b|Removed\b|Config OK:|Statistics reset|Managed tools are ready)|\bis up to date\b|\bupdated to\b|\binstalled and will be used\b").is_match(&value) {
        role = "success";
    }
    if re!(r"(?i)^(?:\[?FAIL\]?\s|Error:)|Status:\s*(?:failed|cancelled)").is_match(&value)
        || (re!(r"(?i)^veo: ").is_match(&value) && !re!(r"(?i)^veo: note:").is_match(&value))
    {
        role = "error";
    }
    if re!(r"Status:\s*saved").is_match(&value) {
        role = "success";
    }
    if re!(r"^[1-9][0-9]* problems? found").is_match(&value) {
        role = "error";
    }
    if value.starts_with("No problems found") {
        role = if re!(r"\b[1-9][0-9]* warnings?\b").is_match(&value) { "warn" } else { "success" };
    }
    if value.starts_with("Playback note:") || value.starts_with("Warning:") {
        role = "warn";
    }
    if value.starts_with("Config reset:") {
        role = "success";
    }
    if value.starts_with("Summary:") {
        role = if re!(r"\b[1-9][0-9]* failed\b|cancelled").is_match(&value) { "error" } else { "success" };
    }
    style(tty, &link_output_paths(tty, original, &line), role, enabled)
}

/// Something that owns live terminal rows and must clear them before prose.
pub trait LiveRegion: Send + Sync {
    fn before_output(&self);
}

/// A destination for application prose: stdout or stderr, formatted for a
/// terminal unless it is redirected or `--json` asked for plain output.
#[derive(Clone)]
pub struct Out {
    pub std: Std,
    pub tty: bool,
    pub plain: bool,
    pub color: bool,
    live: Option<Arc<dyn LiveRegion>>,
}

impl Out {
    pub fn new(std: Std, plain: bool) -> Self {
        Out { std, tty: std.is_tty(), plain, color: terminal::color_enabled(), live: None }
    }

    pub fn stdout(plain: bool) -> Self {
        Out::new(Std::Out, plain)
    }

    pub fn stderr(plain: bool) -> Self {
        Out::new(Std::Err, plain)
    }

    /// Output for a subcommand: `--json` keeps it plain, color flags win.
    pub fn command(std: Std, args: &[String]) -> Self {
        let mut out = Out::new(std, args.iter().any(|arg| arg == "--json"));
        if let Ok(display) = output_options(args) {
            if let Some(color) = display.color {
                out.color = color;
            }
        }
        out
    }

    pub fn with_live(mut self, live: Arc<dyn LiveRegion>) -> Self {
        if self.tty {
            self.live = Some(live);
        }
        self
    }

    pub fn styled(&self) -> bool {
        self.tty && !self.plain
    }

    pub fn write(&self, text: &str) {
        if let Some(live) = &self.live {
            live.before_output();
        }
        if !self.styled() || text.contains('\x1b') {
            self.std.write(text);
        } else {
            self.std.write(&format_output(text, true, self.color));
        }
    }

    pub fn line(&self, text: impl AsRef<str>) {
        self.write(&format!("{}\n", text.as_ref()));
    }
}

/// Remove the color flags a subcommand's own parser does not know.
pub fn strip_color_flags(args: &[String]) -> Vec<String> {
    args.iter().filter(|arg| *arg != "--no-color" && *arg != "--color").cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_display_options() {
        let args: Vec<String> = ["stats", "--profile", "plain", "--no-color", "--json"].iter().map(|s| s.to_string()).collect();
        let options = output_options(&args).unwrap();
        assert_eq!(options.profile.as_deref(), Some("plain"));
        assert_eq!(options.color, Some(false));
        assert_eq!(options.remaining, vec!["stats", "--no-color", "--json"]);
        assert!(output_options(&["--profile".to_string()]).is_err());
    }

    #[test]
    fn plain_text_is_unchanged_without_terminal() {
        let text = "veo stats\n\nTracking since: x\n";
        assert_eq!(format_output(text, false, true), text);
    }
}
