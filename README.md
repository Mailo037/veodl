# veo

A small, clean video-downloading CLI powered by [yt-dlp](https://github.com/yt-dlp/yt-dlp). No banners, just progress and the saved file path.

Automating with an agent? Start with the [agent guide](docs/AGENT_GUIDE.md) for JSON output, run inspection and audio verification.

**Only download content you own or are authorized or legally permitted to download.** Respect copyright, website terms, and access restrictions. veo does not bypass DRM, private-content access controls, or paywalls.

## Install and run

Requires **Node.js 22+** and npm. Internet access is needed for installation and first-use backend setup.

The npm package is `veodl`; the installed commands are `veo` and `veodl`.

```bash
npx veodl "https://example.com/video.mp4"
```

Or install globally:

```bash
npm install -g veodl
veo "https://example.com/video.mp4"
# or: veodl "https://example.com/video.mp4"
```

The first download automatically prepares missing yt-dlp, FFmpeg and FFprobe.
On Android/Termux this includes installing the native packages and yt-dlp's
JavaScript support with `pkg install -y`. On desktop systems, missing media tools
are downloaded into veo's own cache. Run `veo doctor fix` to prepare everything
before downloading. Plain `veo doctor`, help and version do not install tools.

From this checkout, without publishing:

```bash
npm install
node bin/veo.js --help
npm link
veo "https://example.com/video.mp4"
```

`package.json` maps `"veo": "./bin/veo.js"`; npm creates the executable/shim automatically. No custom command prefix is needed. Use only trusted URLs, and quote URLs to protect query strings from your shell.

If something does not work, run `veo doctor` first: it inspects the whole local setup and prints one line per check.

Run `veo` without arguments in an interactive terminal for a guided download: choose a
profile, enter a link, select video or audio, choose available resolution and output
directory, and start. The wizard checks the link and asks about playlist mode only when
it finds a collection; configured playlist and entry selection values are preselected.
Playlist mode also lists entries for selection. The wizard enables
resume and offers skipping previously downloaded files. With redirected input, use the
regular command arguments; veo never starts a prompt in a script.

## Usage

```text
veo <url> [<url>...] [options]

Options:
  -q, --quality <quality>   best, 2160p, 1440p, 1080p, 720p, 480p, 360p
  -o, --output <path>       Output directory (default: current working directory)
  -r, --rename <name>       Filename without extension; * inserts the original title
  --closest-quality         Nearest available resolution instead of an upper bound
  --open                    Open the saved file with your default app
  --audio                   Audio only (MP3 by default)
  --format <format>         Video: mp4, mkv, webm, mov
                            Audio: mp3, m4a, aac, opus, flac, wav
  --recode                  Explicit video conversion (requires --format)
  --playlist-concurrency <n> Parallel playlist entries, 1-4 (default: 2)
  --playlist                Download every entry of a playlist or channel URL
  -N, --concurrent-fragments <n>   Parallel fragments, 1-64 (default: 8; 17-64 experimental)
  --experimental-fragments  Explicitly accept experimental values above 16
  --subs                    Download subtitles (default languages: en)
  --sub-langs <langs>       Subtitle languages, e.g. "de,en" (implies --subs)
  --embed-subs              Embed subtitles into the video file
  --embed-metadata          Embed title, date and other metadata
  --embed-thumbnail         Embed the thumbnail
  --sponsorblock-remove <categories>   e.g. "sponsor,selfpromo"
  --section <range>         Only a time range, e.g. "*10:00-12:00"
  --cookies <file>          Netscape cookie file, for content you may access
  --cookies-from-browser <browser[:profile]>
  --resume                  Keep partial data and continue an interrupted download
  --list-qualities          Show available video resolutions and exit
  --list-formats            Show available formats and exit
  --dry-run                 Show what would be downloaded and exit
  --json                    One JSON object per URL instead of prose
  --profile <name>          Use a named profile from the config file
  --batch-file <file>       Read one URL per line (blank lines and # comments ignored)
  --retry-failed <id|file>  Retry failed/unfinished downloads by run ID or job file
  --playlist-items <list>   Select entries, e.g. 1,3-5 (implies --playlist)
  --skip-existing          Skip matching downloads that are still on disk
  --no-<boolean-option>    Disable a default, e.g. --no-open or --no-audio
  -v, --version             Show installed version (also: veo version)
  -h, --help                Show help
```

```bash
veo "https://youtube.com/watch?v=VIDEO_ID" -q 1080p
veo "https://x.com/USER/status/STATUS_ID" -q best
veo "https://example.com/video.mp4" --audio
veo "https://example.com/video.mp4" --audio --format flac
veo "https://example.com/video.mp4" --format webm --recode -o ./videos
veo "https://example.com/video.mp4" -r "My Video" --format mp4 --open
veo "https://example.com/video.mp4" --audio -r "My Music"
veo <url1> <url2> <url3> --embed-metadata --subs
veo "https://youtube.com/playlist?list=LIST_ID" --playlist -q 720p
veo "https://example.com/video.mp4" --resume
npx veodl "https://example.com/video.mp4" --output ./downloads
```

### Several URLs and playlists

Playlist downloads use two concurrent entries by default; use `--playlist-concurrency 1` for sequential downloads. Each entry uses up to eight concurrent DASH/HLS fragments by default (`-N 1` disables fragment parallelism). Fragment counts between 1 and 16 need no confirmation. Values from 17 to 64 are experimental and prompt for `y/N` confirmation in terminals, or require `--experimental-fragments` (or `"experimentalFragments": true` in configuration/profiles) in non-interactive environments and JSON mode. Values above 64 are rejected. More connections help only when the source and connection have spare capacity, and excessive connections may trigger server limits. Video format changes preserve the encoded streams unless `--recode` is explicitly enabled; audio extraction and precise section cuts retain their existing conversion behavior.

- Pass any number of URLs; up to two run concurrently by default. Use
  `--concurrent-downloads 1` for sequential processing. A failure is reported
  with its URL and does not stop the remaining URLs; the exit status is `1` if anything
  failed.
- Playlists, channels and other collections are refused by default with a hint. Add
  `--playlist` to download every entry. Each entry keeps its own title; `--rename` applies
  a name prefix per entry (`My Name - 001`), or substitutes each title with `movie_*`.
- Use `--playlist-items 1,3-5` to select entries by their original, one-based index.
  Playlist entries run with bounded parallelism: a failed entry does not discard successful
  files or stop the remaining entries. Before downloading, veo displays the selected count
  and a size estimate when the source supplies sizes. Unknown sizes are labeled explicitly;
  metadata estimates do not predict conversion size.
- `--batch-file links.txt` accepts a UTF-8 URL list, optionally alongside URLs on the command
  line. A final summary counts saved, skipped and failed videos. Successful files are opened
  with `--open` even if another URL or playlist entry fails.
- Failed or cancelled jobs print a ready-to-use `veo --retry-failed <runId>` command. Saved job file paths are also accepted.
- Use `veo retry --last` to retry the newest failed or unfinished job without copying its path. Active runs are ignored; use `veo runs` to inspect them.
  Jobs are stored under the per-user veo cache's `jobs` directory. Retries retain resolved
  output directories and settings, even from a different working directory, and explicit
  flags can override them. Completed playlist jobs retry only failed indices; interrupted
  playlists revisit their selection and skip completed entries through resume history.
  Playlist indices refer to the playlist's current order, so changes to that order can
  change what an index selects. Cookie paths and browser-session settings are not saved in
  retry jobs; pass those flags again if needed. Job files contain URLs, settings and local
  file paths and can be deleted when no longer needed.

```bash
veo --batch-file links.txt --profile archive
veo "https://example.com/playlist" --playlist-items 1,3-5 --resume
veo "https://example.com/playlist" --playlist --skip-existing
veo --retry-failed "C:\path\to\job.json"
veo --retry-failed abc123
veo retry --last
```

### Quality

- Numeric qualities are an **upper bound**: `-q 720p` downloads the best resolution at or
  below 720p, so a metered connection can never accidentally receive a 2160p file. If a
  source offers nothing at or below the request, veo stops before downloading and lists
  the resolutions that do exist.
- `--closest-quality` restores the former "nearest available resolution" rule, which may
  pick a resolution *above* the request. It needs a numeric `--quality`.
- Sources that report no resolution metadata fall back to the best available stream.
  Collections resolve and enforce the quality limit separately for each entry.
- Other positive numeric resolutions, such as `-q 540p`, are also accepted. The interactive
  wizard offers the resolutions reported by the source; without resolution metadata it
  offers `best` rather than inventing available streams.
- `--quality` does not apply to audio.
- MP4-compatible codecs are preferred at the selected resolution; merged video prefers MP4
  with MKV fallback. A single-file source may retain its original container. Use
  `--format mp4` to require MP4 through lossless remuxing. Incompatible codecs cause an error.
- `--format` on video remuxes without re-encoding. Add `--recode` to explicitly allow conversion, which may be slow or lossy. Audio formats require `--audio`.

### Playback compatibility

H.264/AAC sources are preferred at the selected quality, including explicit MP4 output.
Original codecs are retained by default; a playback note identifies media needing extra
player codecs. MP4 is a container, not a promise of a particular codec.

Use `veo URL --compatible` to ensure MP4 with H.264 8-bit 4:2:0 and AAC.
Already compatible streams are copied; only incompatible streams are encoded (H.264 CRF 18,
AAC 192 kbit/s), which takes time and may lose quality. HDR requiring video conversion
is refused rather than silently losing correct colors. Embedded subtitles/thumbnails are
not supported in this mode; separate subtitle files remain supported.
New configs include a `kompatibel` profile; existing configs receive a commented profile
example through `veo config edit`. Enable it with `veo URL --profile kompatibel`.

### Download controls and diagnostics

- URL lists and batch files use `--concurrent-downloads 2` by default (range 1–4).
  Each playlist independently uses `--playlist-concurrency 2` (range 1–4).
  The limits multiply when several playlists run at once. JSON emits one complete
  object per URL in completion order; the `url` field identifies each result.
- Adaptive concurrency is on by default. After the backend's own retries are exhausted,
  HTTP 429/5xx and temporary connection failures trigger at most two further attempts,
  after 2 and 4 seconds. Fragment parallelism and future URL/playlist workers are reduced
  for the rest of the run. Active downloads finish normally; format and quality stay fixed.
  Partials are continued on retry. Use `--no-adaptive-concurrency` to disable this.
- `--check-space` checks cache and destination free space before media transfer. Known
  sizes include conservative room for merge output and a destination copy, plus space
  reserved by concurrent downloads in the same process. Unknown sizes are reported as
  unknown; conversion sizes and other processes' disk usage cannot be predicted exactly.
  Use `--no-check-space` to disable this conservative check.
- `--timings` shows phase times at the end of the run. Single downloads show
  timing without a summary; multiple downloads show timing below the summary.
  The phases cover setup, metadata, download/backend, processing, saving and retry waits.
  Download/backend includes the backend's own startup and extraction overhead;
  these are summed per-download times, not a diagnosis of network speed.
  JSON results include `timings` in milliseconds, or `entryTimings` for playlists.
  Use `--no-timings` to hide/omit them.
- Terminal details are gray, titles bold, saves green and errors red. The same styling
  applies to stats, history, runs/stop, doctor, flush, updates, config prose, help and
  interactive prompts. For example, `veo stats --no-color` disables it for one command. Redirected output
  and JSON have no color escapes. `--no-color`, `"color": false`, `NO_COLOR`,
  and `TERM=dumb` disable colors.
  Terminal reports shorten long URLs, paths and detail values to the current window
  width. Titles can use up to three lines before an ellipsis is added. This also
  applies with `--no-color`; JSON and redirected output retain complete values.
  Downloads stay in the normal terminal flow. Only the current progress block is
  updated in place; narrow windows put the statistics below the progress bar.
  Existing logs remain in scrollback and are not replayed or rewritten on resize.
  This can be set independently per profile, for example
  `"profiles": { "default": { "color": true }, "plain": { "color": false } }`.
  Use `veo stats --profile plain` or `veo URL --profile plain`; `--color` or
  `--no-color` overrides the profile for one invocation (except `NO_COLOR`).
  External npm/editor output and Node runtime warnings keep their own formatting.

### Filename templates and folders

`--filename-template` controls the name without extension; `--folder-template`
creates relative subfolders under `--output`. Both support `{title}`, `{id}`,
`{channel}`, `{year}`, `{playlist}` and `{index}` (three-digit playlist index).
Missing metadata uses readable fallbacks. `{year}` is the upload year when available.
Subtitles follow the media name. Existing files are never overwritten. Templates cannot
escape the output folder, and symlink/junction subfolders are refused. Use forward slashes
for folders. `--rename` and `--filename-template` are mutually exclusive.

```powershell
veo "VIDEO-URL" --folder-template "{channel}/{year}" --filename-template "{title} - {id}"
veo "PLAYLIST-URL" --playlist --folder-template "{playlist}" --filename-template "{index} - {title}"
veo config check
veo config check --profile fast
veo config show --profile fast
```

Config checks validate every effective profile (or the selected one), including unknown
settings, ranges and conflicting options. Show prints merged built-in, global and profile
settings as JSON, with credential paths/browser profiles redacted. Neither command contacts
video sites. All new options are also documented in `veo config guide`.

### Files, names and resume

- Original titles are preserved by default. Use `-r "My Video"` or `--rename "My Video"`
  for video or audio. Supply the name **without an extension**; the actual media extension
  is appended automatically. Use `-o` for the directory. Invalid filename characters are
  sanitized and overly long names shortened. Existing files are never intentionally
  overwritten: duplicates get ` (1)`, ` (2)`, etc.
- Use `-r "movie_*"` or `"rename": "movie_*"` in your config/profile to insert the
  original title: `My Film` becomes `movie_My Film.mp4`. Every `*` is substituted.
  Quote the pattern in your shell. Patterns also work with URL batches and playlists.
- Subtitles and thumbnails are saved beside the media file under the same base name
  (`My Video.mp4` → `My Video.en.vtt`).
- Downloads and media processing first finish in the local per-user veo cache, under
  `downloads` (`%LOCALAPPDATA%\veo\downloads` on Windows). The destination is only written
  after the media is ready. Saving uses a hard link when supported or an exclusive copy
  across drives/cloud mounts; copied file sizes are checked. Existing files are never
  overwritten. The local original is removed only after files and history are saved.
  Local disk space is therefore needed for the complete download and processing files.
- If saving fails or is cancelled after media processing completes, the local original and
  sidecars are kept for **15 minutes**, even without `--resume`. Repeat the same command or
  use the printed `--retry-failed` command to retry the transfer. Individual completed
  downloads can be recovered without contacting their source again. The same source,
  media settings and destination identify a cached transfer; changing those starts a new
  download. Expired copies are removed on the next veo invocation, not by a background
  timer while veo is closed. Active transfers are never expired. Another failed transfer
  starts a fresh 15-minute retention period.
- `--resume` also keeps unfinished downloads in local `.veo-part-<request-hash>` folders
  until resumed; unfinished data has no automatic expiry. Without `--resume`, unfinished
  downloads are discarded on failure or cancellation. The hash separates source URLs,
  playlist entries, destinations, quality, media type, format, sections and subtitle/metadata
  options. A manifest records backend-confirmed
  completion and saved files, so an unprocessed file is not mistaken for a finished video.
  Each playlist entry has its own state. Completed entries survive later failures and are
  skipped when resuming the same selection. Partial downloads are never resumed without
  `--resume`. Legacy partial folders in the output directory are left untouched; the new
  local cache does not automatically migrate them.
- A lock prevents two resume processes from using the same partial folder. Normal failures
  and Ctrl+C release it. After a force kill, remove the named `.lock` file only after making
  sure no veo process still uses that folder.
- Successful downloads record their source, output settings and saved paths in
  `.veo-history` inside the output directory. `--skip-existing` uses these records and checks
  that the files still exist. A different quality/format or deleted output is downloaded
  again. Files downloaded before this history existed are not recognized automatically.
  Deleting history removes duplicate detection, not downloaded media. Records whose files
  no longer exist are removed on the next download into that folder, and `.veo-history`
  itself is removed with its last record; `veo doctor` reports stale records.

### Metadata and subtitles

- `--subs` writes subtitle files, `--sub-langs de,en` selects languages, and `--embed-subs`
  embeds them into the video container instead.
- `--embed-metadata` and `--embed-thumbnail` use FFmpeg to write metadata and cover art.
- `--sponsorblock-remove sponsor,selfpromo` cuts sponsor segments (YouTube) and requires
  FFmpeg; the cut re-encodes the affected parts.
- `--section "*10:00-12:00"` downloads only a time range.
- `-N 8` downloads several fragments in parallel, which is noticeably faster on HLS/DASH
  sources and heavier on the network.

### Access and credentials

veo downloads publicly accessible content. It does not bypass access controls, and it does
not expose options that defeat them. If you need your own session for content you are
authorized to view:

- `--cookies ./cookies.txt` passes a Netscape-format cookie file. The file is validated
  before any network work, and on Unix a file readable by other users produces a warning.
- `--cookies-from-browser firefox:Work` reads cookies from an installed browser: `brave`,
  `chrome`, `chromium`, `edge`, `firefox`, `opera`, `safari`, `vivaldi`, `whale`, optionally
  with `+gnomekeyring`/`+kwallet`/`+basic` and a profile or container.

Credentials are used for that single invocation only and are never stored by veo. When a
download fails because a login is required, the error message points at these flags.

### Inspection and scripting

- `veo <page-url> --list-sources` opens the page in a temporary headless Chrome,
  Edge or Chromium profile, repeatedly checks for visible Play controls (including
  controls added later or inside frames), and watches media requests after interaction.
  Use `--json` for numbered source
  metadata. Each result includes its title, host, HLS/DASH/direct type, known
  resolutions, estimated MiB size and Mbit/s bitrate when the source provides them. Signed media
  URLs are not printed. Set `VEO_BROWSER_PATH` if the browser is not found.
- Add `--deep-scan` to verify every media candidate observed during the source
  search. The normal search verifies at most 30 candidates. `--timeout 30s`
  sets a deadline for browser observation and candidate verification; plain
  numbers mean seconds, and `m` means minutes (5 seconds to 10 minutes).
  Defaults are 45 seconds, or 2 minutes with `--deep-scan`. If the deadline
  is reached, verified sources found so far are shown with a timeout notice.
  A deep scan covers observed requests; it cannot discover sources that the
  page never loads. The timeout does not stop a subsequent download.
- For an unsupported single page URL, veo searches for media automatically and
  uses the only verified source when there is one. If several sources are found,
  the terminal asks which one to use; scripts receive a numbered source list and
  a nonzero exit. `veo <page-url> --source 2 --dry-run --json` previews a chosen
  source, and `veo <page-url> --source 2 --json` downloads it. Scripts never
  receive an interactive prompt.
  Retry jobs retain the page URL and selected number and discover a fresh media
  URL, because player links may expire. Browser discovery uses a temporary
  profile without copying browser cookies, and cannot unlock DRM content.
  Misspelled flags and commands show a nearby valid spelling, for example
  `--deepscan` suggests `--deep-scan`.
- `veo <url> --list-qualities` lists available video resolutions, highest first,
  without downloading media. `--list-qualitys` is an alias. Add `--json` for
  a `qualities` array (e.g. `["1080p", "720p"]`); use `--quality 720p` to select
  an upper bound. Unknown resolutions are reported without guessing. Audio-only,
  storyboard and DRM formats are excluded. Supports `--source <number>` for embedded players.
- `--list-formats` prints the backend's own format table for one URL and exits.
- `--dry-run` prints the title, the resolved quality and the destination path that would be
  used — without creating the output directory or downloading anything.
- `--json` prints one JSON object per line on stdout:
  `{"url":…,"status":"saved","title":…,"files":[…]}` or
  `{"url":…,"status":"failed","error":…}`. Progress and status still go to stderr. Without
  `--json`, every saved file is printed as `Saved: <path>` on stdout.
  Duplicate detection can return `status: "skipped"`; cancellation returns `"cancelled"`.
  Playlist results also include `saved`, `skipped` and `failures` (with original indices).
  A failed or cancelled playlist can still report files saved before the failure.
- `veo runs [id] --json` reports active run metadata and per-item progress. Finished attempts
  remain available through `veo history --json`.
- `veo inspect <file> --json` reports local container and stream metadata. Add
  `--check-audio` to decode all audio tracks and measure whether any peak exceeds
  -60 dBFS. A track can exist without a detectable signal. The check reads the full
  media file and uses only local FFmpeg/FFprobe tools.
- Download results and history retain a `runId`. `veo inspect run <id> --json` reads
  a finished run and checks every saved file before probing it. Renamed media is
  found by file identity, size and a sampled SHA-256 fingerprint within the original output tree;
  add `--search <directory>` after moving it elsewhere. Ambiguous or missing files
  are reported without probing another file. Old runs lack this persistent record.
  Finished run records live in the current device's veo cache. On Windows, macOS,
  Linux and Termux, `inspect` uses bundled media tools where supported and otherwise
  uses local `ffmpeg` and `ffprobe` (or `VEO_FFMPEG_PATH`); it does not install them.
  Copying only the media to another device does not copy its run ID record.

### Config file

Defaults can be stored in a config file, so a long list of flags is not needed for every
call. `veo doctor` prints the exact path; `VEO_CONFIG` overrides it.
The file accepts JSON with `//` line comments and `/* ... */` block comments. Strings
(including URLs and Windows paths) keep their normal JSON escaping rules. Trailing commas
are not allowed.

- Windows: `%APPDATA%\veo\config.json`
- macOS: `~/Library/Application Support/veo/config.json`
- Linux: `$XDG_CONFIG_HOME/veo/config.json` (or `~/.config/veo/config.json`)

```json
{
  "output": "D:\\Videos",
  "quality": "1080p",
  "embedMetadata": true,
  "subLangs": "de,en",
  "concurrentFragments": 4
}
```

Supported keys: `output`, `quality`, `format`, `rename`, `audio`, `open`, `resume`,
`closestQuality`, `cookies`, `cookiesFromBrowser`, `playlist`, `concurrentFragments`,
`experimentalFragments`, `playlistConcurrency`, `recode`, `compatible`, `concurrentDownloads`, `adaptiveConcurrency`,
`filenameTemplate`, `folderTemplate`, `checkSpace`, `timings`, `color`,
`subs`, `subLangs`, `embedSubs`, `embedMetadata`, `embedThumbnail`, `sponsorblockRemove`,
`section`, `json`. An explicit command-line flag always wins over a stored default. An
unknown key produces a warning; invalid JSON or a wrong value type is an error, because
silently ignoring a typo would be worse.

Source discovery defaults are `deepScan` (boolean), `timeout` (string such as
`"30s"` or `"2m"`), `listSources` (boolean) and `autoListSources` (boolean).
`listSources: true` makes a single URL list sources and exit; use
`--no-list-sources` to download for one invocation. `autoListSources` defaults
to `true` and searches when ordinary extraction finds no downloadable video.
Set it to `false` or pass `--no-auto-list-sources` to disable that fallback.
A single verified source is selected automatically; multiple sources require
a choice. `deepScan` and
`timeout` apply to either kind of source search.

Additional defaults are `skipExisting` and `playlistItems`. Boolean defaults can be disabled
with `--no-open`, `--no-audio`, `--no-resume`, `--no-embed-metadata`, etc. `--no-subs` also
disables stored subtitle languages and subtitle embedding for that invocation.

### Named profiles

If no profile is selected, `profiles.default` is applied automatically. `veo profile music`
selects another named profile for future commands and stores it as `activeProfile` in the
config; `veo profile` shows the current choice, and `veo profile list` marks it in
the list. `veo profile reset` removes the stored choice and returns to `default`.
You can also use `veo profile default` to select it explicitly.
The names `list` and `reset` are reserved for these commands and cannot be used as profile names.
`--profile NAME` overrides this choice for one command. Named profiles use global defaults
rather than inheriting `default`. The wizard preselects the active profile.
New configurations include an empty `default` profile and the example profiles. Existing configurations are
left unchanged when opened. An empty profile does not change download behavior.

```json
{
  "output": "D:\\Videos",
  "profiles": {
    "default": { "quality": "1080p", "resume": true },
    "music": { "audio": true, "format": "mp3", "output": "D:\\Music" },
    "archive": { "quality": "1080p", "embedMetadata": true, "subLangs": "de,en" }
  }
}
```

`veo <url> --profile music` merges global defaults, then the selected profile, then explicit
CLI flags. `veo config profiles` lists profile names; `veo config path` prints the file path.
The download status shows `Profile: NAME` (or `global (no profile)` when none is configured).
`veo config edit` creates a small, valid configuration for new or empty files. Opening an
existing configuration does not add or update comments. The current option guide is kept in
the program; `veo config guide` prints it without changing the file. In the built-in editor,
F3 offers **A** to add or update the guide at the top and **R** to remove a previously
generated guide. Ctrl+S saves that choice. This can also clean up the long duplicate block
inserted by older versions. Personal settings and notes below a generated block are preserved.
In an interactive terminal, it opens the built-in editor
unless `VISUAL` or `EDITOR` is configured (an executable path, without shell arguments).
The editor provides syntax colors, line numbers and live validation. Syntax errors mark
the affected line; unknown properties and invalid values are highlighted directly, with
spelling suggestions, expected types and allowed values in the status area. Validation uses
the same rules as the CLI, including profile overrides and audio/video formats.
Press F2 on a property or value to open suggestions, use Up/Down to choose, Enter to apply
or Esc to cancel. Suggestions require valid JSON syntax; free-text values and custom numeric
resolutions such as `900p` remain supported. Ctrl+S validates and saves;
Ctrl+Z undoes edits, Ctrl+Y redoes them, and Ctrl+A selects the whole file.
Esc or Ctrl+Q exits, asking before discarding changes. Use arrows, Home/End and PageUp/PageDown
to navigate. Invalid configurations cannot be saved. `--no-color` disables syntax colors.
In terminals supporting SGR mouse reporting, left-click positions the cursor and dragging
selects text, including across lines. Holding the pointer at the top or bottom while dragging
scrolls the file and extends the selection. Backspace/Delete removes the selection; typing replaces
it. The mouse wheel scrolls without changing the editing position; Ctrl+Up/Down also scrolls.
Ctrl+C copies selected text to the system clipboard, and Ctrl+V pastes clipboard text at the
cursor or replaces the selection. Ctrl+Q or Esc exits the editor. The editor disables mouse
reporting again when it exits. Keyboard editing remains available in terminals without mouse
support.
Clipboard access uses the system clipboard on Windows, `pbcopy`/`pbpaste` on macOS, and
`wl-clipboard` (Wayland), `xclip` or `xsel` (X11) on Linux. Linux requires one of those tools
and access to a graphical session. The editor reports when no clipboard is available; terminal
paste shortcuts can still insert text directly when supported by the terminal.
On Termux, editor clipboard shortcuts use `termux-clipboard-set`/`termux-clipboard-get`
when the Termux:API app and `termux-api` package are installed.

Use `veo config edit --external` to use `VISUAL`, then `EDITOR`, then Notepad on Windows
or `vi` elsewhere. `veo config edit --terminal` explicitly selects the built-in editor.
Set `VEO_CONFIG_EDITOR=external` to disable the built-in editor by default (`auto` restores
automatic selection, `terminal` forces it). For a persistent Windows preference:

```powershell
[Environment]::SetEnvironmentVariable('VEO_CONFIG_EDITOR', 'external', 'User')
$env:VEO_CONFIG_EDITOR = 'external'
```

Non-interactive sessions use the external editor unless `--terminal` is explicitly selected,
in which case an interactive-terminal error is reported. The wizard also offers configured profiles.
Generated guides and app messages are in English. Existing comments, profile names and
settings are preserved when opening the editor.

### Terminal output

- During a download, the terminal/tab title shows `veo | 50% | My Video` (original title or
  your `-r` name), plus setup/processing status. It ends with `Done`, `Failed`, or
  `Cancelled`; the shell may replace it at the next prompt. This uses the native console
  title on Windows (including PowerShell/Windows Terminal) and OSC title sequences on
  compatible Linux/macOS terminals. Titles are not changed when stderr is redirected or
  `TERM=dumb`. Terminal settings that enforce a fixed tab title can override this feature.
- Progress shows percentage, speed, downloaded/total size and ETA on stderr; unknown values
  appear as `?`. Separate audio/video streams each have their own progress. Non-interactive
  output is throttled.
- Progress labels include the current item (`[3/12]`), source title, video/audio/media stream,
  postprocessing (merging, conversion, subtitles or metadata) and saving. Stream percentages
  describe that stream, not the entire multi-step job. Sources without codec metadata use
  the neutral `Media` label.
- `--open` launches the completed file in your default app, including renamed files and
  audio. With several URLs, the primary file of each URL is opened. Uses `explorer.exe` on
  Windows, `open` on macOS, and `xdg-open` on Linux (requires a graphical desktop,
  xdg-utils and a file association). The CLI does not wait for the player to close. If the
  opener cannot be launched, a warning is printed and the successful download still exits
  with `0`; later errors inside the detached opener/player are not monitored.
- After each successful command, veo checks the npm registry at most once per day for a
  newer version and prints a one-line notice on stderr (`Update available: veo X
  (you have Y). Run: veo up`). Help, version and the update commands themselves
  stay quiet. Disable it with `VEO_NO_UPDATE_CHECK=1`;
  `VEO_REGISTRY`/`npm_config_registry` are respected.
- Exit status is `0` on success, `1` on errors, and `130` on cancellation. With several
  URLs, `1` means at least one URL failed.

## Commands

```bash
veo version           # installed version (also: veo --version or veo -v)
veo                   # interactive download wizard (terminal only)
veo config edit       # create/open config; F3 offers the optional guide
veo config guide      # print the current option guide
veo config reset      # confirm with y; back up config and restore the current template
veo config profiles   # list available profiles
veo profile            # show the currently selected profile
veo profile list       # list all profiles and mark the active one
veo profile music      # use music as the default for future commands
veo profile reset      # return to default (or global settings if absent)
veo config path       # show the config file location
veo doctor            # diagnose the local setup; exit 1 if a check fails
veo doctor fix        # restore missing or damaged managed tools
veo doctor --offline  # skip the network checks
veo stats             # persistent download totals; --json for scripting
veo history           # the last 5 downloads; --json for scripting
veo history --failed --limit 20  # recent failed/cancelled attempts
veo retry --last       # retry the newest failed or unfinished job
veo runs              # active runs with their id; veo runs <id> for details
veo runs --json       # machine-readable active run metadata
veo inspect FILE --check-audio --json  # track metadata and audio signal
veo inspect run ID --check-audio --json  # finished run and saved media
veo stop [id]         # stop one run, or every active run
veo flush             # stop runs, clear temporary downloads and retry jobs
veo flush --stats     # the same, and reset the statistics
veo update            # install the latest veo with npm
veo update --check    # only check for a newer veo
veo up                # short alias for veo update (also: veo up --check)
veo upgrade           # alias for veo update
veo check update      # alias for veo update --check
veo backend update    # install the newest yt-dlp release
veo backend reset     # back to the release pinned in this veo version
veo alias list        # shipped (veo, veodl) plus custom wrappers
veo alias add veo-dl  # create veo-dl as a wrapper that calls veo
veo alias remove veo-dl  # delete a custom wrapper
veo uninstall -p veodl   # delete one alias (veo itself stays installed)
veo uninstall            # plan the full removal (aliases, package, cache, config)
veo uninstall --yes      # remove everything
```

`veo update` runs `npm install -g veodl@latest` and then removes yt-dlp backend
caches from older pinned releases, keeping an explicitly installed backend release. It
never uses a shell on Linux/macOS, passes fixed arguments only, and prints the manual npm
command on any failure. The registry can be overridden with `VEO_REGISTRY` (or npm's
`npm_config_registry`) for mirrors and proxies.

### `veo alias` and `veo uninstall`

The npm package is `veodl`; a global install ships the commands `veo`
and `veodl`. `veo alias` manages extra command names without reinstalling:

```bash
veo alias list
veo alias add veo-dl
veo alias remove veo-dl
veo uninstall -p veodl
```

`add` creates a small wrapper next to the `veo` command found on `PATH` that
forwards to `veo`, so it survives `veo update`. Names use 2-31 lowercase
letters, digits or hyphens. `remove` (also `veo alias rm`) deletes the wrapper
files; `veo` itself can never be removed this way. Removing a shipped name such
as `veodl` only deletes that shortcut — the next `veo update` recreates it.

`veo uninstall -p <name>` is a shortcut for `veo alias remove <name>`. Bare
`veo uninstall` removes everything: without flags it only prints the plan
(package `veodl` with commands veo, veodl, plus custom wrappers, cache
and config paths); `veo uninstall --yes` deletes all of it — custom wrappers,
the package with `npm uninstall -g veodl`, veo's cache directory (downloads,
jobs, backend tools, history, statistics) and the config file. Use
`--keep-aliases`, `--keep-cache` or `--keep-config` to preserve one part.
The plan shows the active config path, including a `VEO_CONFIG` override.
Add `--json` for scripting.

### `veo flush`

Run `veo flush` to stop active veo runs started with this version, then remove
temporary local downloads, including the 15-minute retained files and unfinished
resume data, and cached retry job JSON files. Those jobs can no longer be retried.
Saved media, output history, config/profiles, the `veo history` list, finished-run
records and backend binaries are kept.
Cleanup waits for cancellation; if a run cannot stop, it fails without deleting
download or job files. Locked folders from older or interrupted processes are
skipped and reported. Only veo's own per-user cache is cleaned. Statistics are
preserved unless `--stats` is given: `veo flush --stats` resets them too.
To end runs without removing their downloads and jobs, use `veo stop [id]` instead.

### `veo stats`

Shows saved videos/audio, failed attempts, skips, cancellations and total time spent
on download requests (including preparation, processing and saving). Parallel run
times are added together. Playlist entries count individually; retries are new
attempts. Active requests are recorded when they finish. Tracking starts with this
version; previous downloads are not imported. `veo stats --json` returns the totals
as JSON. Statistics contain counters and timestamps, not URLs or filenames.

### `veo history`

Shows the **last 5 download attempts** by default, newest first, with title, status, media type,
date, duration, URL and the saved files. Saved, skipped, failed and cancelled items are
recorded, including the reason a failure was reported; playlist entries and retried
attempts count individually. Active downloads appear once they finish. Long file lists
are summarized in the text view.

Use `veo history --limit N` to show 1-1000 attempts, or `veo history --failed` to
show only failed and cancelled attempts; the flags can be combined. Failed attempts
include their exact retry-job command when one was recorded. New attempts also show
their persistent run ID for `veo inspect run <id>`. `veo history --json`
prints `{"count":N,"entries":[…]}` for scripting. Each entry has
`at`, `url`, `title`, `status`, `media` (`video`/`audio`), `quality`, `format`, the
complete `files` list, `error`, `elapsedMs` and an optional `job` path. Titles and paths are stored without
terminal control characters.

History is one small JSON file per attempt in the per-user veo cache's `history`
directory, so parallel runs cannot overwrite each other and each record stays small. The
directory keeps one file per attempt and contains URLs, titles and local file paths;
delete it to remove those records. `veo flush` keeps the list (only `veo flush --stats`
resets statistics, not history).

### `veo runs` and `veo stop`

Every run registers itself while it works, under a **6-character id**, and removes that
record when it ends:

```bash
veo runs            # active runs: id, PID, state, start time, progress, output directory
veo runs k3f9qa     # one run in detail: URLs, settings, job file, per-item state
veo stop k3f9qa     # ask that run to stop and wait until it exits
veo stop            # stop every active run
```

`veo runs` lists the runs of this user, including those started in another terminal, and a
run disappears from the list as soon as it finishes. Progress such as
`1/3 done, 1 running` is read from the run's job file, which is created before the first
download starts, so a run that is still preparing the backend shows `starting`. `veo stop
<id>` writes a stop request that the run itself polls, so it needs no signals or PIDs and
works the same on every platform. A stopped run exits like Ctrl+C (`130`) and keeps its
partial data and retry job, so the printed `veo --retry-failed` command still works —
unlike `veo flush`, which also removes that data. If a run does not stop within 15 seconds,
`veo stop` reports it and exits with `1`.

A run that crashed without cleaning up leaves its record behind: `veo runs` marks it
`stale` and `veo stop` removes it, with or without its id. Records contain the id, process
id, start time, URLs, output directory and job path — never cookie files, browser sessions
or other credentials. Unknown or damaged record files are ignored instead of breaking veo,
because a single stray file in the cache must never stop later runs.

### `veo doctor`

Prints one line per check: `ok`, `warn` or `fail`. It inspects Node.js, the platform, the
output directory, the backend cache, yt-dlp (including the SHA-256 of the cached binary and
the version it reports), FFmpeg/FFprobe, a system FFmpeg fallback, the config file, leftover
partial downloads, stale duplicate-detection records, the npm registry and the yt-dlp release host. It downloads no backend and
only creates its own probe files plus the backend cache directory. Exit status is `1` when
at least one check fails.

Run `veo doctor fix` to restore missing or damaged managed tools and check again.
It stages bundled FFmpeg/FFprobe and downloads verified yt-dlp when needed. Missing
desktop media tools are installed in veo's cache using npm and the pinned media
packages. On Termux it installs missing native tools with the package manager.
With `--offline`, it only uses local binaries; missing tools are reported for an online retry.
Use `-o PATH` to create and check an output directory. Config errors and invalid overrides
are reported for manual correction; PATH and config values are never rewritten.
A missing system FFmpeg is not a warning when the selected media tools work.

## Supported sites and backend

YouTube, X/Twitter, TikTok, Vimeo, Reddit, Instagram, and [many other yt-dlp sites](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md) are supported **when publicly accessible and technically available**. Support changes with websites, regions, rate limits, and backend versions; it is not a guarantee that every URL will work.

Official standalone yt-dlp is acquired on first download and cached outside the package directory; help, version and plain doctor never download the backend. The release is pinned and SHA-256 verified against hashes shipped with this package. FFmpeg and FFprobe are normally supplied by the optional `ffmpeg-static` and `ffprobe-static` dependencies during npm installation. If these are absent and no local pair is available, veo installs `ffmpeg-static@5.3.0` and a platform-specific `@ffprobe-installer` package in a temporary cache project, checks that both programs run, and saves them in its backend cache. Media packages use npm/upstream HTTPS distribution, not the yt-dlp pinned-hash guarantee. These binaries have their own licenses; see their upstream packages. No Python installation is needed on supported standalone platforms.

The existing Node executable is explicitly enabled as yt-dlp's JavaScript runtime for YouTube. Local yt-dlp configuration and plugins are disabled for predictable execution. Arguments are passed without a shell.

`ffmpeg-static` and `ffprobe-static` publish no Windows ARM64 binaries. veo no longer refuses
that platform: it uses an FFmpeg/FFprobe pair found on `PATH`, including the usual
WinGet, Chocolatey and `C:\ffmpeg\bin` locations, or automatically downloads x64
media tools for Windows 11's x64 emulation.

### Android / Termux

With Node.js 22+ and npm already installed in Termux:

```sh
npm install -g veodl
veo "URL"
```

On Android, veo automatically finds `yt-dlp`, `ffmpeg` and `ffprobe` on PATH
(also checking `$PREFIX/bin`). If tools or the `yt-dlp-ejs` Python module are
missing, it runs `pkg install -y python-yt-dlp yt-dlp-ejs ffmpeg` automatically,
then checks the installed programs before continuing the download. Python is
installed as a dependency. Termux with the pacman package manager uses
`--noconfirm` instead. This needs a working Termux repository and internet access.
No root access is needed. Later runs reuse the installed tools.

Static FFmpeg dependencies are optional; `--omit=optional` can be added to the
npm install command to skip attempting those desktop packages entirely.
The system yt-dlp is maintained by Termux and is
not pinned or hash-verified by veo. Explicit `VEO_YT_DLP_PATH` and
`VEO_FFMPEG_PATH` overrides still take precedence.

`veo doctor fix` also performs this automatic setup; `veo doctor fix --offline`
never invokes a package manager. Update Termux packages with `pkg upgrade`;
`veo backend update` prints this platform's update instructions.

To save files in Android's shared Downloads folder, run `termux-setup-storage`,
grant the storage permission, then use `veo "URL" -o ~/storage/downloads`.
Keep veo and its tools in Termux's private storage.

For existing published veo 1.6.1 installations (before this automatic setup), run:

```sh
pkg install python-yt-dlp yt-dlp-ejs ffmpeg
export VEO_YT_DLP_PATH="$(command -v yt-dlp)"
export VEO_FFMPEG_PATH="$PREFIX/bin"
veo doctor
```

These exports apply to the current shell; add them to your shell startup file
if you need them on subsequent launches of 1.6.1.

### Advanced overrides

- `VEO_YT_DLP_PATH`: absolute path to a trusted, current yt-dlp executable. It always takes
  precedence over the managed backend, including one installed by `veo backend update`.
- `VEO_FFMPEG_PATH`: directory containing both `ffmpeg` and `ffprobe` executables (with
  `.exe` on Windows).
- `VEO_CONFIG`: path to the config file.

Automatic setup covers Windows x64/ia32/ARM64, macOS x64/ARM64, Linux x64/ARM64,
and Android through Termux. Windows ARM64 media tools require x64 emulation.
`--open` uses `termux-open` in Termux; another Android app must be available to view the file.
Other systems should supply trusted native binaries. Node.js 22+, npm, internet
access, executable private storage and working OS libraries are prerequisites;
network blocks or unavailable repositories can still prevent setup. Android's
shared-storage permission must be granted by the user. Maintainers should update
the pinned yt-dlp release and hashes as websites change,
or users can install a newer release themselves.

### Installing a newer backend

```bash
veo backend update --check   # is a newer yt-dlp release available?
veo backend update           # install it
veo backend reset            # forget it, use the pinned release again
```

The pinned backend is hash-verified against values compiled into this veo version. That
guarantee cannot cover a release that did not exist when this version was built, so
`veo backend update` verifies the download against the `SHA2-256SUMS` file published with
that release over HTTPS: the trust anchor becomes HTTPS and GitHub instead of the npm
package. It is opt-in, it says so on stderr when it runs, and `veo backend reset` (plus the
next `veo update`) returns to the pinned release. An installed release is used only while it
is newer than the pinned one and its bytes still match the recorded hash; anything else
silently falls back to the pinned release.

## Develop and publish

```bash
npm install
npm test                 # unit tests, no network
npm run test:smoke       # real downloads: local HTTP server, real yt-dlp, real FFmpeg
npm run test:open        # real download plus a controlled desktop-opener replacement
npm pack
npm run check:package    # inspect the packed tarball
npm link
veo --version
# Then, authenticated as the account that owns the name:
npm publish
```

The smoke test generates a two-second synthetic video with FFmpeg, serves it on loopback,
and runs real yt-dlp downloads. It checks default output, the quality cap, duplicate names,
audio extraction, conversion, dry-run, format listing, JSON output, metadata embedding,
resume, batch behaviour, playlist selection and nonzero failures, and verifies that a
finished download whose destination was unavailable is retried from the local cache
without contacting the source again. It also confirms that real runs leave `veo history`
entries, that finished runs clean up their record, and that `veo runs`/`veo stop` report
nothing left behind. It needs network access once for yt-dlp acquisition; it downloads no
third-party video. Unit tests require no network.

The published tarball only includes `bin/`, `src/`, package metadata, README and LICENSE.
The lockfile is kept for reproducible development. There is no build step. npm makes the
shebang-bearing bin executable on installation (on Unix, `chmod +x bin/veo.js` also enables
direct checkout execution).

Continuous integration runs the unit tests on Windows, macOS and Linux for Node 22 and 24,
runs the end-to-end smoke test on Node 22, and verifies the packed tarball.

Modules separate argument parsing (`src/cli.js`), configuration (`src/config.js`,
`src/config-errors.js`, `src/config-template.js`), backend setup (`src/backend.js`),
backend updates (`src/backend-update.js`), download orchestration (`src/downloader.js`,
`src/download-cache.js`), retry jobs (`src/jobs.js`), playlists (`src/playlist.js`),
statistics, download history, the run registry and cleanup (`src/stats.js`,
`src/history.js`, `src/runs.js`, `src/flush.js`), diagnostics (`src/doctor.js`),
progress (`src/progress.js`), and helpers (`src/utils.js`, `src/paths.js`,
`src/state.js`, `src/version.js`) so new options and providers can be added without
replacing the CLI.

## License

MIT for this CLI. yt-dlp and FFmpeg/FFprobe retain their respective upstream licenses.
# Clickable paths

In supported terminals (Windows Terminal, iTerm2, WezTerm, Kitty, VS Code and recent VTE terminals), the `Saved:` path, `veo config path` and the config editor's header link to the containing folder. Use the terminal's link gesture, usually Ctrl+click or Cmd+click. The terminal and operating system decide whether file links open the file manager; unsupported terminals and redirected output show plain paths. Missing paths are not linked. JSON output remains plain.
