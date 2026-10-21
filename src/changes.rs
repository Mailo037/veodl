use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::output::Out;
use crate::suggest::suggest;
use crate::terminal::{self, word_wrap};
use crate::utils::clean_text;
use crate::version::{self, CURRENT};

/// The CHANGELOG.md of this build, so notes always match the installed code.
pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");
const MARKER: &str = "(installed)";
const SHOWN: usize = 5;
const USAGE: &str = "Usage: veo changes [<version>] [--json] [--latest] [--since <version>] [--to <version>] [--range <from>..<to>]";

pub const CHANGES_HELP: &str = "veo changes [<version>] [--json] [--latest] [--since <version>]
                          [--to <version>] [--range <from>..<to>]

List every released version, newest first, and mark the installed one. With a version
(or --version) show that release's notes; a partial version such as 1.10 selects the
newest match. --since shows everything newer than a version, --to and --range set both
ends of a range, and --latest shows only the newest release. Notes come from the
CHANGELOG.md shipped with this veo version, so the command needs no network access and
prints no automatic update hint. Use --json for scripting.
";

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Item {
    pub text: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    #[serde(rename = "type")]
    pub kind: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub sections: Vec<Section>,
}

/// Keep a Changelog markdown → releases, newest first. A second-level heading
/// that is not a version ends the notes.
pub fn parse_changelog(markdown: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    // (top-level item index, nested index) of the bullet a wrapped line continues.
    let mut last: Option<(usize, Option<usize>)> = None;
    for raw in markdown.split('\n') {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            last = None;
            continue;
        }
        if let Some(heading) = re!(r"^##\s+v?([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?)(?:\s+[-–—]\s+([0-9]{4}-[0-9]{2}-[0-9]{2}))?\s*$").captures(line) {
            releases.push(Release { version: heading[1].to_string(), date: heading.get(2).map(|date| date.as_str().to_string()), sections: Vec::new() });
            last = None;
            continue;
        }
        if re!(r"^##\s").is_match(line) {
            break;
        }
        let Some(release) = releases.last_mut() else { continue };
        if let Some(kind) = re!(r"^###\s+(.+?)\s*$").captures(line) {
            release.sections.push(Section { kind: clean_text(&kind[1]), items: Vec::new() });
            last = None;
            continue;
        }
        let Some(section) = release.sections.last_mut() else { continue };
        if let Some(bullet) = re!(r"^(\s*)-\s+(.*)$").captures(line) {
            let text = clean_text(&bullet[2]);
            if text.is_empty() {
                continue;
            }
            if bullet[1].is_empty() || section.items.is_empty() {
                section.items.push(Item { text, items: Vec::new() });
                last = Some((section.items.len() - 1, None));
            } else {
                let parent = section.items.len() - 1;
                section.items[parent].items.push(Item { text, items: Vec::new() });
                last = Some((parent, Some(section.items[parent].items.len() - 1)));
            }
            continue;
        }
        // Without a bullet the line continues the previous one.
        if let Some((parent, nested)) = last {
            let item = match nested {
                Some(nested) => &mut section.items[parent].items[nested],
                None => &mut section.items[parent],
            };
            item.text = clean_text(&format!("{} {}", item.text, line.trim()));
        }
    }
    releases.sort_by(|a, b| version::cmp(&b.version, &a.version));
    releases
}

// Keep a Changelog order; unknown section types follow in file order.
const SECTION_ORDER: [&str; 6] = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"];

fn ordered_sections(release: &Release) -> Vec<&Section> {
    let rank = |kind: &str| SECTION_ORDER.iter().position(|known| *known == kind).unwrap_or(SECTION_ORDER.len());
    let mut sections: Vec<(usize, &Section)> = release.sections.iter().filter(|section| !section.items.is_empty()).enumerate().collect();
    sections.sort_by(|(left_index, left), (right_index, right)| rank(&left.kind).cmp(&rank(&right.kind)).then(left_index.cmp(right_index)));
    sections.into_iter().map(|(_, section)| section).collect()
}

pub fn release_counts(release: &Release) -> Map<String, Value> {
    ordered_sections(release).into_iter().map(|section| (section.kind.clone(), Value::from(section.items.len()))).collect()
}

fn summary(counts: &Map<String, Value>) -> String {
    counts.iter().map(|(kind, count)| format!("{kind} {count}")).collect::<Vec<_>>().join(", ")
}

/// Exact match first, then the newest release starting with the given prefix.
pub fn resolve_version<'a>(releases: &'a [Release], requested: &str, label: &str) -> Result<&'a Release> {
    let wanted = requested.trim();
    let wanted = wanted.strip_prefix(['v', 'V']).unwrap_or(wanted);
    if !re!(r"^[0-9]+(\.[0-9]+)*$").is_match(wanted) {
        return Err(Error::msg(format!("{label} must be a version number such as 1.11.0.")));
    }
    if let Some(exact) = releases.iter().find(|release| release.version == wanted) {
        return Ok(exact);
    }
    let partial = releases.iter().filter(|release| release.version.starts_with(&format!("{wanted}.")) || release.version.starts_with(&format!("{wanted}-")));
    if let Some(best) = partial.max_by(|a, b| version::cmp(&a.version, &b.version)) {
        return Ok(best);
    }
    let known: Vec<String> = releases.iter().map(|release| release.version.clone()).collect();
    if known.is_empty() {
        return Err(Error::msg(format!("{label} \"{}\" cannot be resolved: the changelog lists no releases.", clean_text(requested))));
    }
    let guess = suggest(wanted, &known).map(|guess| format!(" Did you mean \"{guess}\"?")).unwrap_or_default();
    let list = known.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    Err(Error::msg(format!("Unknown {label} \"{}\".{guess} Known versions: {list}{}.", clean_text(requested), if known.len() > SHOWN { ", …" } else { "" })))
}

#[derive(Default)]
pub struct ChangesOptions {
    pub json: bool,
    pub latest: bool,
    pub version: Option<String>,
    pub since: Option<String>,
    pub to: Option<String>,
}

pub struct Selection<'a> {
    pub mode: &'static str,
    pub from: Option<String>,
    pub to: Option<String>,
    pub versions: Vec<&'a Release>,
}

pub fn select_releases<'a>(releases: &'a [Release], options: &ChangesOptions) -> Result<Selection<'a>> {
    let mut sorted: Vec<&Release> = releases.iter().collect();
    sorted.sort_by(|a, b| version::cmp(&b.version, &a.version));
    let newest = sorted.first().map(|release| release.version.clone());
    if let Some(requested) = &options.version {
        let release = resolve_version(releases, requested, "version")?;
        return Ok(Selection { mode: "version", from: Some(release.version.clone()), to: Some(release.version.clone()), versions: vec![release] });
    }
    if options.latest {
        return Ok(Selection { mode: "latest", from: newest.clone(), to: newest, versions: sorted.into_iter().take(1).collect() });
    }
    if options.since.is_some() || options.to.is_some() {
        let from = options.since.as_ref().map(|since| resolve_version(releases, since, "--since").map(|release| release.version.clone())).transpose()?;
        let upper = options.to.as_ref().map(|to| resolve_version(releases, to, "--to").map(|release| release.version.clone())).transpose()?;
        if let (Some(from), Some(upper)) = (&from, &upper) {
            if version::cmp(upper, from).is_lt() {
                return Err(Error::msg(format!("--to {upper} is older than --since {from}.")));
            }
        }
        let versions: Vec<&Release> = sorted
            .into_iter()
            .filter(|release| {
                from.as_ref().is_none_or(|from| version::cmp(&release.version, from).is_gt())
                    && upper.as_ref().is_none_or(|upper| !version::cmp(&release.version, upper).is_gt())
            })
            .collect();
        if versions.is_empty() {
            if let (Some(from), None) = (&from, &upper) {
                return Err(Error::msg(format!("This veo is already at {from}; no newer changes are listed.")));
            }
            return Err(Error::msg(format!(
                "No changes listed between {} and {}.",
                from.as_deref().unwrap_or("the first release"),
                upper.as_deref().or(newest.as_deref()).unwrap_or("")
            )));
        }
        return Ok(Selection { mode: "range", from, to: upper.or(newest), versions });
    }
    Ok(Selection { mode: "list", from: None, to: None, versions: sorted })
}

fn bullet_lines(item: &Item, tty: bool, depth: usize) -> Vec<String> {
    let prefix = format!("{}- ", "  ".repeat(depth));
    let hanging = " ".repeat(prefix.len());
    // Notes are wrapped instead of shortened.
    let columns = tty.then(|| terminal::columns().saturating_sub(prefix.len() + 1).max(24));
    word_wrap(&item.text, columns).split('\n').enumerate().map(|(index, line)| format!("{}{line}", if index > 0 { &hanging } else { &prefix })).collect()
}

fn release_lines(release: &Release, tty: bool, installed: bool, header: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if header {
        lines.push(format!("{}{}", release.version, if installed { format!("  {MARKER}") } else { String::new() }));
    }
    for section in ordered_sections(release) {
        lines.push(section.kind.clone());
        for entry in &section.items {
            lines.extend(bullet_lines(entry, tty, 1));
            for nested in &entry.items {
                lines.extend(bullet_lines(nested, tty, 2));
            }
        }
        lines.push(String::new());
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn plural(count: usize) -> String {
    format!("{count} version{}", if count == 1 { "" } else { "s" })
}

pub fn format_changes(selection: &Selection, current: &str, tty: bool) -> String {
    if selection.versions.is_empty() {
        return "veo changes\n\nNo release notes found in CHANGELOG.md.\n".into();
    }
    if selection.mode == "list" {
        let width = selection.versions.iter().map(|release| release.version.len()).max().unwrap_or(0);
        let mut lines = vec![format!("veo changes ({}, newest first)", plural(selection.versions.len())), String::new()];
        for release in &selection.versions {
            let installed = if release.version == current { MARKER } else { "" };
            let row = format!("  {:<width$}  {:<marker$}", release.version, installed, marker = MARKER.len()).trim_end().to_string();
            let counts = summary(&release_counts(release));
            lines.push(if counts.is_empty() { row } else { format!("{:<pad$}  {counts}", row, pad = width + MARKER.len() + 4) });
        }
        lines.push(String::new());
        lines.push("Use veo changes <version> for one version, veo changes --since <version> for".into());
        lines.push("everything newer than a given version, or --json for scripting.".into());
        return format!("{}\n", lines.join("\n"));
    }
    let title = if selection.mode == "range" {
        let span = match &selection.from {
            Some(from) => format!("{from} to {}", selection.to.as_deref().unwrap_or("")),
            None => format!("up to {}", selection.to.as_deref().unwrap_or("")),
        };
        format!("veo changes {span} ({}, newest first)", plural(selection.versions.len()))
    } else {
        format!("veo changes {}", selection.from.as_deref().unwrap_or(""))
    };
    let mut lines = vec![title, String::new()];
    for release in &selection.versions {
        lines.extend(release_lines(release, tty, release.version == current, selection.mode == "range"));
        lines.push(String::new());
    }
    lines.pop();
    format!("{}\n", lines.join("\n"))
}

fn sections_json(release: &Release) -> Value {
    json!(ordered_sections(release))
}

fn changes_json(selection: &Selection, current: &str) -> Value {
    match selection.mode {
        "list" => json!({
            "current": current,
            "count": selection.versions.len(),
            "versions": selection.versions.iter().map(|release| {
                let counts = release_counts(release);
                let summary = summary(&counts);
                json!({ "version": release.version, "date": release.date, "installed": release.version == current, "counts": counts, "summary": summary })
            }).collect::<Vec<_>>(),
        }),
        "range" => json!({
            "current": current, "from": selection.from, "to": selection.to, "count": selection.versions.len(),
            "versions": selection.versions.iter().map(|release| json!({ "version": release.version, "date": release.date, "installed": release.version == current, "sections": sections_json(release) })).collect::<Vec<_>>(),
        }),
        _ => {
            let release = selection.versions[0];
            json!({ "current": current, "version": release.version, "date": release.date, "installed": release.version == current, "sections": sections_json(release) })
        }
    }
}

pub fn parse_changes_args(args: &[String]) -> Result<ChangesOptions> {
    let mut options = ChangesOptions::default();
    let mut index = 0;
    while index < args.len() {
        let mut arg = args[index].clone();
        let mut inline = None;
        if let Some(equals) = arg.find('=').filter(|equals| arg.starts_with("--") && *equals > 2) {
            inline = Some(arg[equals + 1..].to_string());
            arg.truncate(equals);
        }
        let mut value = |flag: &str| -> Result<String> {
            let value = match inline.take() {
                Some(value) => Some(value),
                None => {
                    index += 1;
                    args.get(index).cloned()
                }
            };
            value
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
                .ok_or_else(|| Error::msg(format!("{flag} requires a version number such as 1.11.0.")))
        };
        match arg.as_str() {
            "--json" => options.json = true,
            "--latest" => options.latest = true,
            "--version" => options.version = Some(value("--version")?),
            "--since" => options.since = Some(value("--since")?),
            "--to" => options.to = Some(value("--to")?),
            "--range" => {
                let range = value("--range")?;
                let (from, to) = range
                    .split_once("..")
                    .filter(|(from, to)| !from.is_empty() && !to.is_empty())
                    .ok_or_else(|| Error::msg(format!("--range must look like 1.9.0..1.11.0. {USAGE}")))?;
                let to = to.split("..").next().unwrap_or(to);
                options.since.get_or_insert_with(|| from.to_string());
                options.to.get_or_insert_with(|| to.to_string());
            }
            other if other.starts_with('-') => return Err(Error::msg(format!("Unknown option \"{other}\". {USAGE}"))),
            other if options.version.is_none() => options.version = Some(other.to_string()),
            _ => return Err(Error::msg(USAGE)),
        }
        index += 1;
    }
    let ranged = options.since.is_some() || options.to.is_some();
    if options.version.is_some() && (ranged || options.latest) {
        return Err(Error::msg(format!("Use either a version or --latest/--since/--to, not both. {USAGE}")));
    }
    if options.latest && ranged {
        return Err(Error::msg(format!("Use --latest on its own. {USAGE}")));
    }
    Ok(options)
}

pub fn changes_main(args: &[String], stdout: &Out) -> Result<i32> {
    let args = crate::output::strip_color_flags(args);
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        stdout.write(CHANGES_HELP);
        return Ok(0);
    }
    let options = parse_changes_args(&args)?;
    let releases = parse_changelog(CHANGELOG);
    let selection = select_releases(&releases, &options)?;
    if options.json {
        stdout.line(serde_json::to_string(&changes_json(&selection, CURRENT))?);
    } else {
        stdout.write(&format_changes(&selection, CURRENT, stdout.styled()));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog\n\nIntro.\n\n## 1.2.0\n\n### Fixed\n\n- A fix that\n  wraps.\n\n### Added\n\n- Feature\n  - Nested detail\n\n## 1.10.0 - 2026-01-02\n\n### Added\n\n- Big\n\n## 1.1.0\n\n### Changed\n\n- Old\n\n## Links\n\n- ignored\n";

    #[test]
    fn parses_sections_and_nested_items() {
        let releases = parse_changelog(SAMPLE);
        assert_eq!(releases.iter().map(|release| release.version.as_str()).collect::<Vec<_>>(), ["1.10.0", "1.2.0", "1.1.0"]);
        assert_eq!(releases[0].date.as_deref(), Some("2026-01-02"));
        let ordered = ordered_sections(&releases[1]);
        assert_eq!(ordered[0].kind, "Added");
        assert_eq!(ordered[0].items[0].items[0].text, "Nested detail");
        assert_eq!(ordered[1].items[0].text, "A fix that wraps.");
    }

    #[test]
    fn selects_versions_and_ranges() {
        let releases = parse_changelog(SAMPLE);
        let range = select_releases(&releases, &ChangesOptions { since: Some("1.1.0".into()), ..ChangesOptions::default() }).unwrap();
        assert_eq!(range.versions.len(), 2);
        let partial = select_releases(&releases, &ChangesOptions { version: Some("1".into()), ..ChangesOptions::default() }).unwrap();
        assert_eq!(partial.versions[0].version, "1.10.0");
        let unknown = select_releases(&releases, &ChangesOptions { version: Some("1.3.0".into()), ..ChangesOptions::default() }).err().unwrap();
        assert!(unknown.message.starts_with("Unknown version \"1.3.0\"."), "{}", unknown.message);
        assert!(select_releases(&releases, &ChangesOptions { since: Some("1.10.0".into()), ..ChangesOptions::default() }).is_err());
    }

    #[test]
    fn formats_the_version_list() {
        let releases = parse_changelog(SAMPLE);
        let selection = select_releases(&releases, &ChangesOptions::default()).unwrap();
        let text = format_changes(&selection, "1.2.0", false);
        assert!(text.contains("  1.2.0   (installed)  Added 1, Fixed 1\n"), "{text}");
        assert!(text.starts_with("veo changes (3 versions, newest first)\n\n"));
    }

    #[test]
    fn shipped_changelog_lists_this_version() {
        assert!(parse_changelog(CHANGELOG).iter().any(|release| release.version == CURRENT));
    }

    #[test]
    fn parses_arguments() {
        let args: Vec<String> = ["--range=1.0.0..1.2.0", "--json"].iter().map(|arg| arg.to_string()).collect();
        let options = parse_changes_args(&args).unwrap();
        assert_eq!((options.since.as_deref(), options.to.as_deref(), options.json), (Some("1.0.0"), Some("1.2.0"), true));
        assert!(parse_changes_args(&["1.0.0".into(), "--latest".into()]).is_err());
    }
}
