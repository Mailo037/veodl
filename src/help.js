// Help text is built from rows so every description starts in the same column.
const COLUMN = 30;

const OPTION_GROUPS = [
  ['Download', [
    ['-q, --quality <quality>', 'Video quality (best, 2160p, 1440p, 1080p, 720p, 480p, 360p)', 'Numeric qualities are an upper bound: -q 720p never', 'downloads 2160p.', true],
    ['--closest-quality', 'Pick the nearest available resolution, above or below', true],
    ['-o, --output <path>', 'Output directory (default: current directory)', true],
    ['-r, --rename <name>', 'Filename without extension; * inserts the original title', true],
    ['--audio', 'Download audio only (default: mp3)', true],
    ['--format <format>', 'Video: mp4, mkv, webm, mov; audio: mp3, m4a, aac, opus, flac, wav', 'Video uses lossless remux; incompatible codecs fail.', true],
    ['--compatible', 'Ensure MP4 H.264/AAC; converts only when needed (may lose quality)'],
    ['--recode', 'Allow video conversion (requires --format; may lose quality)'],
    ['--open', 'Open the saved file with your default app', true],
    ['--resume', 'Keep partial data and continue an interrupted download'],
    ['--skip-existing', 'Skip matching downloads still present on disk'],
    ['--section <range>', 'Download only a time range, e.g. "*10:00-12:00"'],
    ['--sponsorblock-remove <categories>', 'Remove sponsor segments, e.g. "sponsor,selfpromo"'],
  ]],
  ['Subtitles and metadata', [
    ['--subs', 'Download subtitles (default languages: en)', true],
    ['--auto-subs', 'Include automatically generated subtitles'],
    ['--sub-langs <langs>', 'Subtitle languages, e.g. "de,en" (manual unless --auto-subs)'],
    ['--sub-format <format>', 'Preferred subtitle format, e.g. srt/vtt/best'],
    ['--list-subs', 'Show manual and automatic subtitle languages, then exit'],
    ['--embed-subs', 'Embed subtitles into the video file'],
    ['--embed-metadata', 'Embed title, date and other metadata'],
    ['--embed-thumbnail', 'Embed the thumbnail'],
  ]],
  ['Playlists and batches', [
    ['--playlist', 'Download every entry of a playlist or channel URL', true],
    ['--playlist-items <list>', 'Select playlist entries, e.g. 1,3-5 (implies --playlist)'],
    ['--playlist-concurrency <n>', 'Simultaneous playlist downloads (1-4; default: 2)'],
    ['--concurrent-downloads <n>', 'Parallel URLs/batch entries (1-4; default: 2)'],
    ['--batch-file <file>', 'Read URLs from a file (one per line; # comments)'],
    ['--retry-failed <id|file>', 'Retry failed/unfinished items by run ID or job file'],
  ]],
  ['Naming and privacy', [
    ['--filename-template <s>', 'Filename without extension, e.g. {index} - {title}'],
    ['--folder-template <s>', 'Relative folders, e.g. {channel}/{year}'],
    ['--neutral-filename', 'Use a random video name; omit title and naming templates'],
    ['--incognito', 'Save media without run IDs, history, stats or retry data'],
  ]],
  ['Inspect before downloading', [
    ['--list-qualities', 'Show available video resolutions and exit', true],
    ['--list-qualitys', 'Alias for --list-qualities'],
    ['--list-formats', 'Show the available formats and exit'],
    ['--list-sources', 'Find video sources loaded by a web page and exit'],
    ['--auto-list-sources', 'Search after no video is found (default: on)'],
    ['--source <n>', 'Select source number from that page (also for scripts)'],
    ['--deep-scan', 'Check every media candidate found during source search'],
    ['--timeout <duration>', 'Source search deadline, e.g. 30, 30s or 2m (5s-10m)'],
    ['--dry-run', 'Show what would be downloaded and exit', true],
  ]],
  ['Network and performance', [
    ['-N, --concurrent-fragments <n>', 'Parallel fragments (1-64; default: 8); 17-64 are experimental'],
    ['--experimental-fragments', 'Explicitly accept experimental values above 16', 'Otherwise requires terminal confirmation before download'],
    ['--adaptive-concurrency', 'Reduce connections and retry temporary failures (default: on)'],
    ['--check-space', 'Estimate cache/output space before downloading (default: on)'],
    ['--cookies <file>', 'Netscape cookie file, for content you may access'],
    ['--cookies-from-browser <browser[:profile]>', 'Read cookies from an installed browser'],
  ]],
  ['Output and scripting', [
    ['--json', 'Print one JSON object per URL instead of prose', true],
    ['--verify', 'Probe saved media files with FFprobe (default: off)'],
    ['--timings', 'Show phase timings (default: on; --no-timings disables)'],
    ['--no-color', 'Disable terminal colors (also respects NO_COLOR)'],
    ['--profile <name>', 'Apply a named config profile'],
    ['--no-<boolean-option>', 'Disable a stored boolean default, e.g. --no-open'],
    ['-v, --version', 'Show installed version (also: veo version)', true],
    ['-h, --help', 'Show help; veo help all lists every option and command', true],
  ]],
];

const COMMANDS = [
  ['veo doctor', 'Diagnose the local setup', true],
  ['veo update|up [--check]', 'Update veo itself with npm (up is a short alias)', true],
  ['veo history', 'Show the last 5 downloads (--json for scripting)', true],
  ['veo history --failed --limit 20', 'Filter and extend download history'],
  ['veo retry --last', 'Retry the newest failed or unfinished job', true],
  ['veo runs [id]', 'List active runs; add --json for metadata and progress'],
  ['veo stop [id]', 'Stop one run, or every active run'],
  ['veo stats', 'Show persistent download statistics'],
  ['veo changes [<version>]', 'List release notes; --since <version> for newer changes'],
  ['veo subs <url>', 'List available manual and automatic subtitles'],
  ['veo inspect <file>', 'Read media metadata; --check-audio measures audio signal'],
  ['veo inspect run <id>', 'Inspect saved files from a finished run by its id'],
  ['veo backend update', 'Install a newer yt-dlp release (see veo backend --help)'],
  ['veo flush', 'Stop veo runs and clear temporary downloads and jobs'],
  ['veo config edit|path|profiles|guide|check|show|reset', 'Manage defaults and named profiles', true],
  ['veo config edit [--external|--terminal]', 'Choose the configuration editor'],
  ['veo profile [list|reset|name]', 'Show, list or change the default profile'],
  ['veo alias list|add|remove', 'Manage extra command names (wrappers calling veo)'],
  ['veo uninstall [-p <name>]', 'Remove one alias, or everything with --yes'],
  ['veo version', 'Show the installed version'],
];

const EXAMPLES = `Examples:
  veo "https://youtube.com/watch?v=..."
  veo <url> -q 1080p
  veo <url> --audio
  veo <url> -o ./downloads
  veo <url> -r "My Video" --open
  veo <url1> <url2> --subs --embed-metadata
  veo update --check
`;

// A row is [name, description, ...continuation lines, common?]; a name too
// wide for the column puts its description on the following line.
function rows(entries, all) {
  const lines = [];
  for (const entry of entries) {
    const common = entry.at(-1) === true;
    if (!all && !common) continue;
    const [name, ...text] = common ? entry.slice(0, -1) : entry;
    const indent = ' '.repeat(COLUMN);
    const label = `  ${name}`;
    if (label.length > COLUMN - 2) lines.push(label, ...text.map(line => indent + line));
    else lines.push(label.padEnd(COLUMN) + text[0], ...text.slice(1).map(line => indent + line));
  }
  return lines.join('\n');
}

function build(all) {
  const options = all
    ? OPTION_GROUPS.map(([title, entries]) => `${title}:\n${rows(entries, true)}`).join('\n\n')
    : `Options:\n${rows(OPTION_GROUPS.flatMap(([, entries]) => entries), false)}`;
  const more = all
    ? 'Run veo without arguments in a terminal for interactive setup.\nAgent workflow: see docs/AGENT_GUIDE.md in the repository or installed package.\n\nDefaults can be stored in the veo config file; veo doctor prints its location.'
    : 'Run veo help all to list every option and command.\nRun veo without arguments in a terminal for interactive setup.';
  return `veo - simple video downloader

Usage:
  veo <url> [<url>...] [options]

${options}

Commands:
${rows(COMMANDS, all)}

${more}

${EXAMPLES}
Only download content you are authorized or legally permitted to download.
`;
}

export const HELP = build(false);
export const HELP_ALL = build(true);
