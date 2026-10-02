# Changelog

All notable changes to veo. This project follows [Semantic Versioning](https://semver.org/).

## 1.12.1

### Fixed

- Remove the optional FFmpeg npm dependency tree and its installation scripts.
- Download desktop FFmpeg and FFprobe from fixed upstream releases and verify compressed and executable SHA-256 hashes before either program runs. Preserve upstream license and build information alongside the tools.
- Verify managed media caches on reuse and reject legacy, corrupt or symlinked binaries. Preserve trusted system tools and explicit overrides.
- Prevent rejected managed-cache binaries from being accepted again through PATH or a symlink alias; continue searching for external system tools.
- Keep Smolish cookie environment variables out of media-tool probe processes.
- Retry completed local transfers without contacting the original source, including selected-source retries, while retaining cache expiry and path validation.

## 1.12.0

### Added

- **Release Notes**: Added `veo changes [<version>]` (alias `veo changelog`) to list every released version, newest first, with the installed one marked. `veo changes <version>` (or `--version <version>`, and a partial version such as `1.10`) shows the notes of one release, `--latest` shows only the newest, and `--since <version>` with `--to <version>` or `--range <from>..<to>` shows everything between two versions. The notes are parsed from the `CHANGELOG.md` shipped with the installed version, so the command works offline, wraps long entries to the terminal width and prints no automatic update hint. `--json` returns the parsed sections, entries and nested entries unchanged for scripting.

## 1.11.0

### Added

- **Incognito Downloads**: Added `--incognito` (and `"incognito": true` in configuration/profiles) to save media without creating run records, persistent retry jobs, run archives, download history, `.veo-history` records, or download statistics. Uses an isolated temporary staging directory and completely removes it upon completion, failure, or cancellation, and disables yt-dlp's filesystem cache.
- **Neutral Filenames**: Added `--neutral-filename` (and `"neutralFilename": true`) to generate unidentifiable random filenames (e.g. `video-a1b2c3d4e5f6.mp4`), suppressing video titles, channel names, and custom folder templates in filesystem paths.
- **Subtitle Inspection & Automatic Captions**:
  - Added `veo subs <url>` (alias `--list-subs` / `--list-subtitles`) to inspect available manual and automatically generated subtitle tracks, languages, and source formats without downloading.
  - Added `--auto-subs` (and `"autoSubs": true`) to download automatically generated captions/subtitles.
  - Added `--sub-format <format>` (and `"subFormat": string`) to express a preferred subtitle source format (e.g. `srt/vtt/best`).
- **Post-Download Media Verification**: Added `--verify` (and `"verify": true`) to probe saved media files with FFprobe before recording success, verifying container integrity and stream presence. Retains saved files for inspection if verification fails.

## 1.10.1

### Added

- Added `veo <url> --list-qualities` (alias `--list-qualitys`) to list available video resolutions ordered highest first without downloading media. Supports `--json` and discovered `--source <n>`.
- Expanded `--concurrent-fragments` (or `-N`) to support values from 1 to 64. Experimental values between 17 and 64 require interactive `[y/N]` confirmation in terminals, or explicit opt-in via `--experimental-fragments` (or `"experimentalFragments": true` in configuration/profiles) for scripts and automated jobs. Values above 64 are rejected.
- Added support for passing run IDs directly to `--retry-failed` (e.g. `veo --retry-failed <runId>`), automatically locating and resolving the corresponding retry job and rejecting active runs.
- Added clickable OSC 8 directory links for saved file destinations, config paths, and editor headers in supported terminals (Windows Terminal, iTerm2, WezTerm, Kitty, VS Code, and VTE).
- Added inline terminal region support (`createInlineRegion`) to smoothly render live progress and status in place without flickering, entering fullscreen, or wiping terminal scrollback.

### Fixed

- Handled terminal window resize events during live progress display, dynamically refitting titles and status lines.
- Improved terminal title and line wrapping to display up to three lines for long titles before truncating.
- Improved interactive config editor footer layout for long status messages and ensured stdin is paused cleanly upon exit.
- Preserved complete original values in JSON and redirected output while adapting prose output to the active terminal width.

## 1.10.0

### Added

- **Persistent Profile Management**: Added `veo profile [NAME|list|reset]` commands to view the active configuration profile, inspect available profiles, switch defaults, or reset back to default without manual configuration edits.
- **On-demand Config Guide**: Added `veo config guide` to display full configuration guidance, supported options, and templates in the terminal without opening the interactive editor.
- **Interactive Config Editor enhancements**:
  - Full Undo (`Ctrl+Z`) and Redo (`Ctrl+Y`) history support when modifying settings.
  - Select All (`Ctrl+A`) shortcut to easily highlight and overwrite lines or entire sections.
  - Interactive F3 guide management prompt (`A` to add/refresh, `R` to strip documentation lines).
  - Mouse edge drag auto-scrolling when dragging selections past viewport bounds.
- **Automatic Browser Source Discovery**: Enabled automatic source discovery fallback by default when direct extraction fails, automatically selecting and proceeding if a single verified media stream is found.

### Changed

- Streamlined source discovery observation polling to immediately proceed once candidate streams are detected.
- Enhanced progress output styling and ANSI handling across terminal status reporting.

## 1.9.1

### Added

- Added typo and spelling suggestions for misspelled CLI options and subcommands (e.g. `--deepscan` suggests `--deep-scan`, `veo histroy` suggests `veo history`).

### Fixed

- Improved browser-based player source discovery to continuously detect delayed play controls and overlays inside frames.

## 1.9.0

### Added

- **Browser-based player source discovery**: Added `veo <page-url> --list-sources` to launch a temporary headless Chrome, Edge or Chromium session, simulate playback, and extract embedded media streams (HLS, DASH, direct MP4/WebM).
- Added `--source <n>` to download or preview (`--dry-run`) a specific discovered source.
- Added `--deep-scan` and `--timeout <duration>` (e.g. `30s`, `2m`) to configure source candidate search depth and deadlines.
- Added interactive source discovery prompt in terminals when direct URL extraction finds no downloadable video.
- Added `autoListSources`, `listSources`, `deepScan`, and `timeout` settings to config defaults and profiles.
- **Config editor improvements**: Enhanced terminal layout with dynamic multi-line detail rows, compact status display, and automatic refresh of the marked template guide for new options.

### Fixed

- Resolved active configuration path properly when running `veo uninstall`.
- Ensured run cancellation requests are processed once per run to avoid race conditions.

## 1.8.3

### Added

- Added `veo alias` commands (`list`, `add`, `remove`) to manage custom command names and shortcuts that forward to `veo`.
- Added `veo uninstall` to remove individual aliases with `-p <name>`, or perform a full interactive/planned purge with `--yes`.
- Added `veo up` as a short alias for `veo update`.

### Changed

- Streamlined package binary commands to `veo` and `veodl`.

## 1.8.2

### Changed

- Updated repository, issue tracker, and homepage URLs to `Mailo037/veodl`.

## 1.8.1

### Changed

- Renamed package to `veodl` on npm.
- Provided `veo`, `veod`, and `veodl` as parallel terminal commands (all execute the same CLI).

## 1.8.0

### Added

- **`veo inspect <file>` and `veo inspect run <id>`**: Inspect local container and stream metadata with `--json`. Add `--check-audio` to decode audio tracks and verify signals above -60 dBFS. `veo inspect run <id>` locates saved run files by file identity, size, and sampled SHA-256 fingerprint, even after renames or moves (`--search <directory>`).
- **`veo retry --last`**: Automatically retry the most recent failed or unfinished job without manually locating its job file.
- **`veo history` improvements**: Added `--limit <1-1000>` to customize history entries and `--failed` to view only failed or cancelled attempts with their retry command and persistent `runId`.
- **Config editor clipboard & mouse scrolling**: Added native system clipboard copy/paste (Ctrl+C / Ctrl+V) across Windows, macOS, Linux (Wayland/X11), and Termux, as well as mouse wheel and Ctrl+Up/Down scrolling.
- **Termux desktop opener**: Supported `--open` in Android/Termux using `termux-open`.
- **Agent guidelines**: Included AGENTS.md and docs/AGENT_GUIDE.md in the distributed package.

## 1.7.2

### Fixed

- Stale download-history records referencing deleted files are automatically pruned on subsequent downloads, and empty `.veo-history` directories are removed.
- `veo doctor` inspects `.veo-history` records, reports stale duplicate-detection records as warnings, and flags damaged history directories.

## 1.7.1

### Fixed

- Progress adapts to narrow terminals, updates one line for parallel downloads, and marks estimated sizes instead of showing premature completion.
- Download errors retain specific HTTP/network codes instead of replacing them with a generic network warning.
- Files announced by a failed yt-dlp process are retained as unconfirmed, not treated as completed transfers; retry requires a successful backend exit.

## 1.7.0

### Added

- Android/Termux automatically uses system yt-dlp and FFmpeg, with platform-specific doctor and backend-update instructions.
- First downloads and `veo doctor fix` automatically install missing Termux tools (including JavaScript support) or acquire missing desktop media binaries in veo's cache; offline mode never installs packages.

### Changed

- Static desktop media tools are optional dependencies so unsupported binaries do not prevent npm installation.

## 1.6.1

### Added

- `veo config reset` restores the current template after confirmation and saves an exact backup of the previous configuration, including malformed files.

## 1.6.0

### Added

- Parallel URL and batch downloads with serialized jobs, per-item statistics, and adaptive retries with reduced concurrency.
- Parallel playlist downloads (two playlist entries concurrently by default, `--playlist-concurrency 1-4`), with per-entry progress, serialized job updates, and cancellation that waits for active workers.
- Concurrent DASH/HLS fragment downloads (up to eight DASH/HLS fragments concurrently by default; `-N` overrides this).
- Safe filename and folder templates, free-space estimates, and phase timings.
- Offline config commands: `veo config check` and `veo config show`, and full option documentation in the config editor.
- Prefer H.264/AAC for explicit MP4 downloads, report incompatible original codecs, and add opt-in `--compatible` / `kompatibel` profile with conditional H.264/AAC conversion.
- `--recode` flag for explicitly converting video formats when remuxing is incompatible.

### Changed

- Video `--format` now remuxes without quality loss; incompatible codecs fail instead of being converted silently.
- Apply consistent terminal styling to all veo command reports and help, including stats; dim terminal status details and highlight titles, successful saves and failures; preserve plain JSON/piped output and honor `--no-color`, `NO_COLOR` and config color preferences.

### Fixed

- Fix Node.js DEP0190 warnings during Windows self-updates by invoking cmd.exe explicitly with a fixed npm command.

## 1.5.0

### Added

- **`veo runs` and `veo stop`**: every run registers itself with a **6-character id** while
  it works, so another terminal can watch and end it. `veo runs` lists active runs with PID,
  state, start time, progress and output directory, `veo runs <id>` shows one run in detail
  (URLs, media settings, job file and per-item state), `veo stop <id>` stops that run and
  `veo stop` stops every run. A stop request is polled by the run itself, so no signals or
  PIDs are needed; the stopped run exits like Ctrl+C and keeps its partial data and retry
  job. Records of crashed runs are marked `stale` and removed by `veo stop`. Run records
  never contain credentials or cookie settings, and an unknown or damaged record file is
  ignored instead of breaking later runs.
- **`veo history`**: shows the last 5 download attempts, newest first, with title, status,
  media type, date, duration, URL and the saved files. Saved, skipped, failed and cancelled
  items are recorded, including the reported failure reason; playlist entries and retries
  count individually. `veo history --json` prints `{"count":N,"entries":[…]}` with the
  complete file list for scripting. Records live as one small file per attempt in the
  per-user veo cache's `history` directory and are kept by `veo flush`.

### Changed

- The run registry moved out of `veo flush` into `src/runs.js`; `veo flush` and `veo stop`
  now stop the same registered runs, and `veo flush` keeps working with run records written
  by older versions.

## 1.4.0

### Added

- **Local download cache**: downloads and media processing now finish in the per-user veo
  cache before the destination is written, so the output location is only touched once the
  media is ready. Saving uses a hard link where supported and an exclusive copy across
  drives or cloud mounts, with copied sizes verified. Existing files are still never
  overwritten. The local original is removed only after files and history are saved.
- **Recoverable transfers**: if saving fails or is cancelled after processing completed, the
  finished local original and its sidecars are kept for **15 minutes** — with or without
  `--resume`. Repeat the same command, or use the printed `--retry-failed` command, to retry
  the transfer without contacting the source again. The same source, media settings and
  destination identify a cached transfer; a repeated failure starts a fresh retention
  period. Expired copies are removed on the next veo invocation, never by a background
  timer, and active transfers are never expired.
- **`veo stats`**: persistent counters for saved videos and audio, failed attempts, skips,
  cancellations and total elapsed download-request time. `veo stats --json` prints the
  totals as JSON. Playlist entries count individually and retries are new attempts.
  Statistics hold counters and timestamps, not URLs or filenames.
- **`veo flush [--stats]`**: stops active veo runs started with this version, then removes
  temporary local downloads — including the 15-minute retained copies and unfinished resume
  data — plus cached retry job files. Saved media, output history, config/profiles and
  backend binaries are kept, and `--stats` additionally resets the statistics.
- **`veo doctor fix`**: restores missing or damaged managed tools and then checks again,
  staging bundled FFmpeg/FFprobe and downloading hash-verified yt-dlp when needed. With
  `--offline` only local binaries are used, and `-o PATH` creates and checks an output
  directory.
- **Title placeholders** in `--rename` and the config/profile `rename` key: `-r "movie_*"`
  inserts the original title (`movie_My Film.mp4`), and every `*` is substituted. Placeholders
  make `--rename` usable with several URLs and playlists, where a plain name is still refused.
- **`profiles.default`** is applied automatically when no profile is selected; other named
  profiles keep using the global defaults instead of inheriting `default`. `veo config edit`
  adds an empty `default` profile to existing configurations without changing settings, and
  the wizard preselects it.
- Config syntax errors now name the line and column and explain the likely mistake, such as
  Markdown code fences, a trailing comma or an unescaped Windows path, without echoing
  private config values.
- The packaged-tarball check now verifies that every relative import resolves inside the
  tarball and that the tarball, `package.json` and CHANGELOG versions agree, so an
  incompletely bumped or incompletely packed release fails before publishing.

### Changed

- `veo doctor` no longer warns about a missing system FFmpeg when the selected FFmpeg and
  FFprobe work, and it reports retained local downloads and copied-size mismatches.
- Resume state and partial data now live in the local per-user cache under `downloads`
  instead of the output directory, and the request hash also separates destinations,
  playlist entries and source URLs. Legacy `.veo-part-<video id>` folders in the output
  directory are left untouched and are not migrated automatically.
- Unfinished downloads are discarded on failure or cancellation unless `--resume` is given;
  a completed download is the only thing retained for a transfer retry. Local disk space is
  therefore needed for the complete download and its processing files.

## 1.3.0

### Added

- **Interactive wizard**: run `veo` without arguments in a terminal to be guided through a
  download — profile, link, video or audio, available resolution, output directory, and
  playlist entry selection. It enables resume and offers to skip already downloaded files.
  Redirected input never starts a prompt.
- **Named profiles** in the config file (`"profiles": { "music": {...} }`) plus
  `veo config edit|path|profiles`. Global defaults are merged first, then the profile
  selected with `--profile`, then explicit flags.
- **Negated boolean flags** such as `--no-open`, `--no-audio` or `--no-resume` switch off a
  stored default for one invocation; `--no-subs` also disables stored subtitle languages
  and subtitle embedding.
- **URL list files** with `--batch-file <file>` (one URL per line, blank lines and `#`
  comments ignored), combinable with URLs on the command line.
- **Durable batch jobs and `--retry-failed <file>`**: failed or cancelled runs print a
  ready-to-use retry command, and retries keep the resolved output directory and settings
  even from another working directory. Completed playlist jobs retry only failed indices.
- **A final summary** per run, counting saved, skipped and failed items. With `--open`,
  successful files are opened even when another URL or playlist entry fails.
- **Playlist entries one at a time**: a failed entry keeps previously saved files and does
  not stop the remaining entries. `--playlist-items 1,3-5` selects entries by original
  one-based index, and the selection count plus available size estimates are shown first.
- **`--skip-existing`** reuses the recorded source and settings history and re-checks that
  the files are still on disk. A different quality or format, or a deleted output, is
  downloaded again.
- **Resume by source and settings hash**: partial data lives in
  `.veo-part-<hash>`, guarded by a lock and a manifest that records backend-confirmed
  completion, so an unprocessed file is never mistaken for a finished video. Each playlist
  entry has its own state; completed entries are skipped on a later run and unfinished ones
  are preserved independently.
- **Labelled phases** in the terminal title and progress output: current item, source title,
  video/audio/media stream, conversion or merge, and saving.
- **Workflow regression tests** and real-backend smoke coverage for URL lists, profiles,
  retry, duplicate detection and playlist selection.

### Changed

- Generated configuration comments, the example profiles (`music`, `archive`),
  documentation and sample filenames are English throughout. Previously generated German
  template comments are translated by the next `veo config edit`, preserving existing
  settings, paths and custom profile names.
- `veo config edit` fills new or empty files with a commented template and gives existing
  files a one-time commented reference guide, without changing stored settings. Config
  files accept `//` line and `/* ... */` block comments.
- Ambiguous "resume folders" are replaced by the source/settings hash layout; only
  backend-confirmed postprocessed media is reused.
- Other positive numeric resolutions, such as `-q 540p`, are accepted instead of only the
  documented list.

## 1.2.0

Everything below ships together: `1.1.0` was prepared but never published, so this
release is the first to contain these changes.

### Changed

- **`--quality` is now an upper bound.** `-q 720p` downloads the best resolution at or
  below 720p and never fetches 2160p instead. If a source offers nothing at or below the
  request, veo stops before downloading and names the resolutions that do exist. The
  historical "nearest available height" rule is still available as `--closest-quality`
  and may pick a resolution above the request. Sources without resolution metadata fall
  back to the best available stream, as before.
- A finished download is now linked into place instead of copied. Staging lives inside
  the output directory, so a hard link is enough; filesystems without hard links still
  fall back to a copy. This removes a full extra read and write per download, roughly
  halves the peak disk usage, and makes saving cancellable.
- Fewer duplicated downloads in `veo update`: an explicitly installed backend release
  is no longer pruned.
- Metadata and media tools are prepared before any network work, so an unusable local
  setup fails before a backend download starts.

### Added

- **Multiple URLs** in one invocation: `veo <url1> <url2> ...`. Downloads run
  sequentially, a failure does not stop the remaining URLs, and the exit status is `1`
  if any URL failed.
- **`--playlist`** to download every entry of a playlist or channel URL. Each entry keeps
  its own title, and `--no-playlist` remains the default.
- **Subtitles**: `--subs`, `--sub-langs <langs>`, `--embed-subs`.
- **Metadata**: `--embed-metadata`, `--embed-thumbnail`, `--sponsorblock-remove <categories>`,
  `--section <range>`, and `-N/--concurrent-fragments <n>` for faster fragmented downloads.
- **`--resume`** keeps partial data in `.veo-part-<video id>` and continues an interrupted
  download, including one that was cancelled with Ctrl+C.
- **`--cookies <file>` and `--cookies-from-browser <browser[:profile]>`** for content you
  are authorized to access. Cookie files are validated before any network work and a
  world-readable file produces a warning.
- **`--dry-run`** reports the title, quality and destination path without downloading.
- **`--list-formats`** prints the backend's own format table.
- **`--json`** prints one JSON object per URL for scripting.
- **`veo doctor`** checks Node.js, the output directory, the backend cache, yt-dlp,
  FFmpeg/FFprobe, the config file, leftover partial downloads and network reachability.
  It downloads nothing and exits `1` when a check fails.
- **`veo backend update [--check]`** installs a newer yt-dlp release without waiting for a
  new veo release, verified against that release's own `SHA2-256SUMS`. `veo backend reset`
  returns to the release pinned and hash-verified at build time.
- **Config file** for defaults (output directory, quality, format, subtitle preferences,
  and so on). Explicit command-line flags always win.
- **Windows on ARM** is no longer rejected: when the bundled static media tools are
  unavailable, veo uses a system FFmpeg installation found on `PATH`.

### Fixed

- `--rename` is applied again; it had stopped affecting the saved filename.

## 1.0.2

- Initial published release.
