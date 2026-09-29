# Working with veo

Read [the agent guide](docs/AGENT_GUIDE.md) before automating downloads or media checks.

- Use `veo <url> --list-qualities --json` to inspect available video resolutions, `veo <url> --dry-run --json` to preview output, then `veo <url> --json` to download authorized content.
- Parse one JSON object per stdout line. Read stderr and the exit code too; a nonzero exit means the request did not fully succeed.
- Use `veo runs --json` or `veo runs <id> --json` for active runs, and `veo history --json --limit 20` for finished attempts. Download JSON and history include the persistent `runId`.
- Use `veo changes --json` to read the release notes of the installed version, `veo changes <version> --json` for one release, and `veo changes --since <version> --json` for everything newer than a version. It works offline and prints no update hint.
- Use `veo inspect <file> --check-audio --json` to distinguish an audio track from a measured signal. The check decodes the full audio track and may take time.
- Use `veo inspect run <id> --json` to inspect files from a finished run. It verifies file presence first and searches the output tree by fingerprint after a rename; use `--search <directory>` after a move outside that tree.
- Use `veo retry --last --json` or `veo --retry-failed <runId> --json` for failed or unfinished jobs. Avoid `veo flush` unless the user asked to remove temporary data and retry jobs.
- Keep scripts non-interactive. `veo` without arguments starts a wizard only in a terminal; fragment concurrency above 16 requires `--experimental-fragments` in scripts.
