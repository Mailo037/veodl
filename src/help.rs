// Help text is built from rows so every description starts in the same column.
const COLUMN: usize = 30;

/// A row is a name, its description lines, and whether short help shows it.
type Row = (&'static str, &'static [&'static str], bool);

const OPTION_GROUPS: &[(&str, &[Row])] = &[
    (
        "Download",
        &[
            (
                "-q, --quality <quality>",
                &["Video quality (best, 2160p, 1440p, 1080p, 720p, 480p, 360p)", "Numeric qualities are an upper bound: -q 720p never", "downloads 2160p."],
                true,
            ),
            ("--closest-quality", &["Pick the nearest available resolution, above or below"], true),
            ("-o, --output <path>", &["Output directory (default: current directory)"], true),
            ("-r, --rename <name>", &["Filename without extension; * inserts the original title"], true),
            ("--audio", &["Download audio only (default: mp3)"], true),
            (
                "--format <format>",
                &["Video: mp4, mkv, webm, mov; audio: mp3, m4a, aac, opus, flac, wav", "Video uses lossless remux; incompatible codecs fail."],
                true,
            ),
            ("--compatible", &["Ensure MP4 H.264/AAC; converts only when needed (may lose quality)"], false),
            ("--recode", &["Allow video conversion (requires --format; may lose quality)"], false),
            ("--open", &["Open the saved file with your default app"], true),
            ("--resume", &["Keep partial data and continue an interrupted download"], false),
            ("--skip-existing", &["Skip matching downloads still present on disk"], false),
            ("--section <range>", &["Download only a time range, e.g. \"*10:00-12:00\""], false),
            ("--fast-cut", &["Fast keyframe-aligned cuts; requires --section (less precise)"], false),
            ("--sponsorblock-remove <categories>", &["Remove sponsor segments, e.g. \"sponsor,selfpromo\""], false),
        ],
    ),
    (
        "Subtitles and metadata",
        &[
            ("--subs", &["Download subtitles (default languages: en)"], true),
            ("--auto-subs", &["Include automatically generated subtitles"], false),
            ("--sub-langs <langs>", &["Subtitle languages, e.g. \"de,en\" (manual unless --auto-subs)"], false),
            ("--sub-format <format>", &["Preferred subtitle format, e.g. srt/vtt/best"], false),
            ("--list-subs", &["Show manual and automatic subtitle languages, then exit"], false),
            ("--embed-subs", &["Embed subtitles into the video file"], false),
            ("--embed-metadata", &["Embed title, date and other metadata"], false),
            ("--embed-thumbnail", &["Embed the thumbnail"], false),
        ],
    ),
    (
        "Playlists and batches",
        &[
            ("--playlist", &["Download every entry of a playlist or channel URL"], true),
            ("--playlist-items <list>", &["Select playlist entries, e.g. 1,3-5 (implies --playlist)"], false),
            ("--playlist-concurrency <n>", &["Simultaneous playlist downloads (1-4; default: 2)"], false),
            ("--concurrent-downloads <n>", &["Parallel URLs/batch entries (1-4; default: 2)"], false),
            ("--batch-file <file>", &["Read URLs from a file (one per line; # comments)"], false),
            ("--retry-failed <id|file>", &["Retry failed/unfinished items by run ID or job file"], false),
        ],
    ),
    (
        "Naming and privacy",
        &[
            ("--filename-template <s>", &["Filename without extension, e.g. {index} - {title}"], false),
            ("--folder-template <s>", &["Relative folders, e.g. {channel}/{year}"], false),
            ("--neutral-filename", &["Use a random video name; omit title and naming templates"], false),
            ("--incognito", &["Save media without run IDs, history, stats or retry data"], false),
        ],
    ),
    (
        "Inspect before downloading",
        &[
            ("--list-qualities", &["Show available video resolutions and exit"], true),
            ("--list-qualitys", &["Alias for --list-qualities"], false),
            ("--list-formats", &["Show the available formats and exit"], false),
            ("--list-sources", &["Find video sources loaded by a web page and exit"], false),
            ("--auto-list-sources", &["Search after no video is found (default: on)"], false),
            ("--source <n>", &["Select source number from that page (also for scripts)"], false),
            ("--deep-scan", &["Check every media candidate found during source search"], false),
            ("--timeout <duration>", &["Source search deadline, e.g. 30, 30s or 2m (5s-10m)"], false),
            ("--dry-run", &["Show what would be downloaded and exit"], true),
        ],
    ),
    (
        "Network and performance",
        &[
            ("-N, --concurrent-fragments <n>", &["Parallel fragments (1-64; default: 8); 17-64 are experimental"], false),
            (
                "--experimental-fragments",
                &["Explicitly accept experimental values above 16", "Otherwise requires terminal confirmation before download"],
                false,
            ),
            ("--adaptive-concurrency", &["Host-local back-off with gradual recovery (default: on)"], false),
            ("--native-http", &["Use Rust for eligible direct HTTP media (default: on)"], false),
            ("--http-connections <n>", &["Rust HTTP connections (1-16; default: 4; shared host budget)"], false),
            ("--staging-dir <path|output>", &["Stage on a chosen drive; output stages beside the destination"], false),
            ("--check-space", &["Estimate cache/output space before downloading (default: on)"], false),
            ("--cookies <file>", &["Netscape cookie file, for content you may access"], false),
            ("--cookies-from-browser <browser[:profile]>", &["Read cookies from an installed browser"], false),
        ],
    ),
    (
        "Output and scripting",
        &[
            ("--json", &["Print one JSON object per URL instead of prose"], true),
            ("--verify", &["Probe saved media files with FFprobe (default: off)"], false),
            ("--timings", &["Show phase timings (default: on; --no-timings disables)"], false),
            ("--no-color", &["Disable terminal colors (also respects NO_COLOR)"], false),
            ("--profile <name>", &["Apply a named config profile"], false),
            ("--no-<boolean-option>", &["Disable a stored boolean default, e.g. --no-open"], false),
            ("-v, --version", &["Show installed version (also: veo version)"], true),
            ("-h, --help", &["Show help; veo help all lists every option and command"], true),
        ],
    ),
];

const COMMANDS: &[Row] = &[
    ("veo doctor", &["Diagnose the local setup"], true),
    ("veo update|up [--check]", &["Update veo itself with npm (up is a short alias)"], true),
    ("veo history", &["Show the last 5 downloads (--json for scripting)"], true),
    ("veo history --failed --limit 20", &["Filter and extend download history"], false),
    ("veo retry --last", &["Retry the newest failed or unfinished job"], true),
    ("veo runs [id]", &["List active runs; add --json for metadata and progress"], false),
    ("veo stop [id]", &["Stop one run, or every active run"], false),
    ("veo stats", &["Show persistent download statistics"], false),
    ("veo changes [<version>]", &["List release notes; --since <version> for newer changes"], false),
    ("veo subs <url>", &["List available manual and automatic subtitles"], false),
    ("veo inspect <file>", &["Read media metadata; --check-audio measures audio signal"], false),
    ("veo inspect run <id>", &["Inspect saved files from a finished run by its id"], false),
    ("veo backend update", &["Install a newer yt-dlp release (see veo backend --help)"], false),
    ("veo flush", &["Stop veo runs and clear temporary downloads and jobs"], false),
    ("veo config edit|path|profiles|guide|check|show|reset", &["Manage defaults and named profiles"], true),
    ("veo config edit [--external|--terminal]", &["Choose the configuration editor"], false),
    ("veo profile [list|reset|name]", &["Show, list or change the default profile"], false),
    ("veo alias list|add|remove", &["Manage extra command names (wrappers calling veo)"], false),
    ("veo uninstall [-p <name>]", &["Remove one alias, or everything with --yes"], false),
    ("veo version", &["Show the installed version"], false),
];

const EXAMPLES: &str = "Examples:
  veo \"https://youtube.com/watch?v=...\"
  veo <url> -q 1080p
  veo <url> --audio
  veo <url> -o ./downloads
  veo <url> -r \"My Video\" --open
  veo <url1> <url2> --subs --embed-metadata
  veo update --check
";

// A name too wide for the column puts its description on the following line.
fn rows<'a>(entries: impl Iterator<Item = &'a Row>, all: bool) -> String {
    let indent = " ".repeat(COLUMN);
    let mut lines = Vec::new();
    for (name, text, common) in entries {
        if !all && !common {
            continue;
        }
        let label = format!("  {name}");
        if label.len() > COLUMN - 2 {
            lines.push(label);
            lines.extend(text.iter().map(|line| format!("{indent}{line}")));
        } else {
            lines.push(format!("{label:<COLUMN$}{}", text[0]));
            lines.extend(text[1..].iter().map(|line| format!("{indent}{line}")));
        }
    }
    lines.join("\n")
}

pub fn build(all: bool) -> String {
    let options = if all {
        OPTION_GROUPS.iter().map(|(title, entries)| format!("{title}:\n{}", rows(entries.iter(), true))).collect::<Vec<_>>().join("\n\n")
    } else {
        format!("Options:\n{}", rows(OPTION_GROUPS.iter().flat_map(|(_, entries)| entries.iter()), false))
    };
    let more = if all {
        "Run veo without arguments in a terminal for interactive setup.\nAgent workflow: see docs/AGENT_GUIDE.md in the repository or installed package.\n\nDefaults can be stored in the veo config file; veo doctor prints its location."
    } else {
        "Run veo help all to list every option and command.\nRun veo without arguments in a terminal for interactive setup."
    };
    format!(
        "veo - simple video downloader\n\nUsage:\n  veo <url> [<url>...] [options]\n\n{options}\n\nCommands:\n{}\n\n{more}\n\n{EXAMPLES}\nOnly download content you are authorized or legally permitted to download.\n",
        rows(COMMANDS.iter(), all)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_share_one_column() {
        let help = build(false);
        assert!(help.contains("  -o, --output <path>         Output directory (default: current directory)"));
        assert!(help.lines().all(|line| line.chars().count() < 120));
        let all = build(true);
        assert!(all.contains("Subtitles and metadata:\n"));
        assert!(all.contains("  --cookies-from-browser <browser[:profile]>\n                              Read cookies"));
    }
}
