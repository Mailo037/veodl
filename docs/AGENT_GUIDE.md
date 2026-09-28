# veo for agents

veo is a command-line video downloader. Use it only for content the user is authorized to download. This guide covers the stable command flow and the difference between download state, media tracks and measured audio signal.

## Fast path

```sh
veo "https://example.com/video" --dry-run --json
veo "https://example.com/video" --json --output ./downloads
veo inspect "./downloads/video.mp4" --check-audio --json
```

For a page with an embedded player that the normal extractor cannot identify,
`veo "<page-url>" --dry-run --json` searches for playable media automatically.
It uses a single verified source without a second command. If several sources
are found, run `veo "<page-url>" --list-sources --json` and choose an `index`,
then use `veo "<page-url>" --source <index> --dry-run --json` before downloading.
Source discovery opens a temporary headless Chromium-based
browser, repeatedly checks for Play controls, and observes media requests after
interaction. It needs Chrome, Edge or Chromium,
or `VEO_BROWSER_PATH`. A found source is verified by the backend; size may be
unknown, and DRM-protected media is excluded. Source numbers are tied to the
page's current discovery run and may change later.

Use `--deep-scan --timeout 2m` with `--list-sources` to check every observed
media candidate up to a two-minute source-search deadline. The JSON result
contains `timedOut: true` if the scan stopped at the deadline; any returned
sources were verified before it stopped. A timed-out listing exits nonzero,
even if it contains partial results. The timeout applies to discovery,
not to a later download.

Set `VEO_NO_UPDATE_CHECK=1` when a background update notice would distract a script. Use an explicit output directory to avoid depending on the agent's current working directory. `--dry-run` plans the download without saving media. It can still inspect the source and prepare backend tools on first use.

Use `veo "<url>" --list-qualities --json` to list video resolutions without
downloading media. The result has `status: "qualities"` and a `qualities` array
ordered highest first, such as `["1080p", "720p"]`. `--list-qualitys` is an alias.
An empty array means the video has no known resolution; audio-only and DRM
formats are excluded. Use `--source <index>` for a discovered player source.

## Output and failures

Download commands write **one JSON object per URL** to stdout with `--json`. Several URLs produce newline-delimited JSON, not a JSON array. Progress, summaries and errors go to stderr. A failed item can still contain saved files, especially within a playlist.

Example success:

```json
{"url":"https://example.com/video","status":"saved","title":"Example","files":["/downloads/Example.mp4"],"runId":"abc123"}
```

Example failure:

```json
{"url":"https://example.com/video","status":"failed","error":"HTTP Error 429","files":[]}
```

These are abbreviated examples; fields such as timing and playlist counts may also be present. Exit code `0` means the command succeeded, `1` means at least one item failed, and `130` means cancellation. Validate both the exit code and each result's `status`; a command can fail before producing any stdout JSON. Never infer success from the presence of a file path alone.

## Inspect runs and results

```sh
veo runs --json
veo runs k3f9qa --json
veo history --failed --limit 20 --json
veo retry --last --json
```

`runs` reports **active** runs, including their ID, state, options, job path and per-item progress. Completed runs disappear from this list. Download JSON and `history` retain the `runId`. `history` reports finished attempts with their outcome and saved file paths. Use `veo --retry-failed <runId>` to retry a failed or unfinished run; a saved `job` path is also accepted as `veo --retry-failed "<job-path>"`. The original retry job must still be present, and retrying an active run by ID is rejected. `veo retry --last` finds the newest retryable job automatically. Running jobs are excluded from `retry --last`.

## Inspect saved media and audio

`veo subs <URL> --json` lists manual and automatic subtitle languages and formats without saving media. Select manual subtitles with `--subs --sub-langs de,en`, generated subtitles with `--auto-subs --sub-langs de,en`, and prefer a source format with `--sub-format srt/vtt/best`. `--verify` optionally probes saved media files before recording success; it is off by default and does not decode the full track.

```sh
veo inspect "/downloads/Example.mp4" --json
veo inspect "/downloads/Example.mp4" --check-audio --json
veo inspect run abc123 --check-audio --json
veo inspect run abc123 --search "/moved/videos" --json
```

`inspect` uses local FFprobe/FFmpeg and never downloads a URL. Its JSON contains `format`, `durationSeconds`, `sizeBytes`, `streams`, `hasAudioTrack` and `hasVideoTrack`. Without `--check-audio`, `audioCheck` is `null`.

With `--check-audio`, veo decodes every audio track from start to end. `audioCheck.hasSignal` is true when at least one track's peak exceeds **−60 dBFS**. `audioCheck.tracks` lists each track's `streamIndex`, `maxDbfs` and `hasSignal`; digital silence may have `maxDbfs: null` or a very low level after conversion. A present audio track does not prove audible content. This is a signal-level check, not a listening or speech-quality test. Decoding long media can take time; if FFmpeg fails, the command exits nonzero instead of reporting silence.

`inspect run` reads the saved run record even after the download finishes. It checks whether each file still exists **before** probing it. If a media file was renamed, veo searches the original output tree using file identity, size and a three-part SHA-256 fingerprint. `--search` adds one more directory after a move. Missing or ambiguous matches are reported without probing a different file; missing files make the command exit with code `1`. The fingerprint lives in veo's run history, so this works across media formats without rewriting the media. Runs created before this feature do not have a persistent run record.

The run record is stored in veo's per-user cache on the device that ran the download. `inspect` works on Windows, macOS, Linux and Termux with locally available FFmpeg/FFprobe; set `VEO_FFMPEG_PATH` to their directory when they are not on the normal search path. Moving only the media to another device does not transfer the run record.

## Operational notes

- `veo <URL> --incognito --json` saves media without a run ID, retry job, history, run archive, output duplicate record or statistics. It disables yt-dlp's filesystem cache. A config profile can set `"incognito": true`; `--no-incognito` overrides it. Incognito cannot use `--resume`, `--skip-existing`, or retry. Its temporary staging data is removed after the attempt; the output file and records outside veo (such as shell history or server logs) still exist.
- Add `--neutral-filename` or `"neutralFilename": true` to use random media filenames instead of titles. This option is independent of `--incognito` and overrides custom file and folder naming for that request.

- Fragment concurrency defaults to 8. Values 1-16 need no confirmation; 17-64 are experimental and require terminal `y/N` confirmation before downloading. Scripts and JSON calls must explicitly accept with `--experimental-fragments` or `"experimentalFragments": true` in the config/profile. Values above 64 are always rejected. More connections may trigger server limits and do not guarantee higher speed. Dry-runs and source/format listings do not require confirmation.
- Example profile: `"experimental": { "concurrentFragments": 32, "experimentalFragments": true }`. Run with `veo <URL> --profile experimental`; use `--no-experimental-fragments` to revoke a stored opt-in.

- Run `veo doctor --offline` to check the local setup without network repair. `veo doctor fix` may install missing tools.
- Use `--resume` for interrupted downloads and `--skip-existing` when repeated saves should be avoided.
- `veo flush` removes temporary downloads and retry jobs; do not run it as routine maintenance in an agent workflow.
- Do not call bare `veo` in an interactive terminal from automation: it opens a wizard. Use explicit URLs and flags.
