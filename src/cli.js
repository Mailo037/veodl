import { outputOptions, outputStream, withOutputSettings } from './output.js';
import { validateFragments, confirmExperimentalFragments } from './experimental-fragments.js';
import { validateTemplate } from './naming.js';
import { parseArgs } from 'node:util';
import path from 'node:path';
import { readFile } from 'node:fs/promises';
import { createInterface } from 'node:readline/promises';
import { createReporter } from './progress.js';
import { openFile } from './open-file.js';
import { maybeUpdateNotice, updateMain, UPDATE_HELP, defaultRegistry, packageVersion } from './updater.js';
import { applyProfile, configMain, loadConfig, profileMain, selectedProfileName } from './config.js';
import { validateItems, describeEstimate } from './playlist.js';
import { parseSourceTimeout } from './source-discovery.js';
import { explainUnknownOption, optionSpellings, suggestOption } from './option-suggestions.js';
import { retryOptions, runJob, jobFilePath } from './jobs.js';
import { QUALITIES, VIDEO_FORMATS, AUDIO_FORMATS, validateUrl, readableError, cleanText, validateCookieFile, validateBrowserSpec, cookieFileWarning } from './utils.js';

export const HELP = `veo - simple video downloader

Usage:
  veo <url> [<url>...] [options]

Options:
  -q, --quality <quality>   Video quality (best, 2160p, 1440p, 1080p, 720p, 480p, 360p)
                           Numeric qualities are an upper bound: -q 720p never
                           downloads 2160p. Use --closest-quality for the nearest
                           available resolution instead.
  -o, --output <path>       Output directory (default: current directory)
  -r, --rename <name>       Filename without extension; * inserts the original title
  --closest-quality         Pick the nearest available resolution, above or below
  --open                   Open the saved file with your default app
  --audio                  Download audio only (default: mp3)
  --format <format>        Video: mp4, mkv, webm, mov; audio: mp3, m4a, aac, opus, flac, wav
                           Video uses lossless remux; incompatible codecs fail.
  --compatible             Ensure MP4 H.264/AAC; converts only when needed (may lose quality)
  --recode                 Allow video conversion (requires --format; may lose quality)
  --concurrent-downloads <n>  Parallel URLs/batch entries (1-4; default: 2)
  --adaptive-concurrency   Reduce connections and retry temporary failures (default: on)
  --filename-template <s>  Filename without extension, e.g. {index} - {title}
  --folder-template <s>    Relative folders, e.g. {channel}/{year}
  --check-space            Estimate cache/output space before downloading (default: on)
  --timings                Show phase timings (default: on; --no-timings disables)
  --no-color               Disable terminal colors (also respects NO_COLOR)
  --playlist-concurrency <n>  Simultaneous playlist downloads (1-4; default: 2)
  --playlist               Download every entry of a playlist or channel URL
  -N, --concurrent-fragments <n>
                           Parallel fragments (1-64; default: 8); 17-64 are experimental
  --experimental-fragments  Explicitly accept experimental values above 16
                           Otherwise requires terminal confirmation before download
  --subs                   Download subtitles (default languages: en)
  --auto-subs              Include automatically generated subtitles
  --sub-langs <langs>      Subtitle languages, e.g. "de,en" (manual unless --auto-subs)
  --sub-format <format>    Preferred subtitle format, e.g. srt/vtt/best
  --list-subs              Show manual and automatic subtitle languages, then exit
  --embed-subs             Embed subtitles into the video file
  --embed-metadata         Embed title, date and other metadata
  --embed-thumbnail        Embed the thumbnail
  --sponsorblock-remove <categories>
                           Remove sponsor segments, e.g. "sponsor,selfpromo"
  --section <range>        Download only a time range, e.g. "*10:00-12:00"
  --cookies <file>         Netscape cookie file, for content you may access
  --cookies-from-browser <browser[:profile]>
                           Read cookies from an installed browser
  --resume                 Keep partial data and continue an interrupted download
  --incognito              Save media without run IDs, history, stats or retry data
  --neutral-filename       Use a random video name; omit title and naming templates
  --list-qualities         Show available video resolutions and exit
  --list-qualitys          Alias for --list-qualities
  --list-formats           Show the available formats and exit
  --list-sources           Find video sources loaded by a web page and exit
  --auto-list-sources      Search after no video is found (default: on)
  --source <n>             Select source number from that page (also for scripts)
  --deep-scan              Check every media candidate found during source search
  --timeout <duration>     Source search deadline, e.g. 30, 30s or 2m (5s-10m)
  --dry-run                Show what would be downloaded and exit
  --verify                 Probe saved media files with FFprobe (default: off)
  --json                   Print one JSON object per URL instead of prose
  --profile <name>         Apply a named config profile
  --batch-file <file>      Read URLs from a file (one per line; # comments)
  --retry-failed <id|file> Retry failed/unfinished items by run ID or job file
  --playlist-items <list>  Select playlist entries, e.g. 1,3-5 (implies --playlist)
  --skip-existing         Skip matching downloads still present on disk
  --no-<boolean-option>   Disable a stored boolean default, e.g. --no-open
  -v, --version            Show installed version (also: veo version)
  -h, --help               Show help (also: veo help)

Commands:
  veo update|up [--check]   Update veo itself with npm (up is a short alias)
  veo backend update       Install a newer yt-dlp release (see veo backend --help)
  veo doctor               Diagnose the local setup
  veo flush                Stop veo runs and clear temporary downloads and jobs
  veo stats                Show persistent download statistics
  veo history              Show the last 5 downloads (--json for scripting)
  veo retry --last         Retry the newest failed or unfinished job
  veo history --failed --limit 20  Filter and extend download history
  veo changes [<version>]   List release notes; --since <version> for newer changes
  veo runs [id]            List active runs; add --json for metadata and progress
  veo inspect <file>        Read media metadata; --check-audio measures audio signal
  veo subs <url>            List available manual and automatic subtitles
  veo inspect run <id>     Inspect saved files from a finished run by its id
  veo stop [id]            Stop one run, or every active run
  veo alias list|add|remove  Manage extra command names (wrappers calling veo)
  veo uninstall [-p <name>]  Remove one alias, or everything with --yes
  veo version              Show the installed version
  veo config edit|path|profiles|guide|check|show|reset  Manage defaults and named profiles
  veo profile [list|reset|name]  Show, list or change the default profile
  veo config edit [--external|--terminal]      Choose the configuration editor

Run veo without arguments in a terminal for interactive setup.
Agent workflow: see docs/AGENT_GUIDE.md in the repository or installed package.

Defaults can be stored in the veo config file; veo doctor prints its location.

Examples:
  veo "https://youtube.com/watch?v=..."
  veo <url> -q 1080p
  veo <url> --audio
  veo <url> -o ./downloads
  veo <url> -r "My Video" --open
  veo <url1> <url2> --subs --embed-metadata
  veo update --check

Only download content you are authorized or legally permitted to download.
`;

const STRING_OPTIONS = {
  quality: { short: 'q' },
  output: { short: 'o' },
  rename: { short: 'r' },
  format: {},
  'playlist-concurrency': {},
  'concurrent-downloads': {},
  'filename-template': {},
  'folder-template': {},
  cookies: {},
  'cookies-from-browser': {},
  'concurrent-fragments': { short: 'N' },
  'sub-langs': {},
  'sub-format': {},
  'sponsorblock-remove': {},
  section: {},
  profile: {},
  'batch-file': {},
  'retry-failed': {},
  'playlist-items': {},
  source: {},
  timeout: {},
};

const BOOLEAN_OPTIONS = {
  'experimental-fragments': {},
  compatible: {},
  recode: {},
  'adaptive-concurrency': {},
  'check-space': {},
  timings: {},
  color: {},
  open: {},
  audio: {},
  resume: {},
  incognito: {},
  'neutral-filename': {},
  playlist: {},
  subs: {},
  'auto-subs': {},
  'list-subs': {},
  verify: {},
  'embed-subs': {},
  'embed-metadata': {},
  'embed-thumbnail': {},
  'closest-quality': {},
  'list-qualities': {},
  'list-qualitys': {},
  'list-formats': {},
  'list-sources': {},
  'auto-list-sources': {},
  'deep-scan': {},
  'dry-run': {},
  json: {},
  help: { short: 'h' },
  version: { short: 'v' },
  'skip-existing': {},
};

// Config defaults, so an explicit flag always wins but a stored preference does not.
export function optionDefaults(config = {}) {
  return {
    quality: config.quality ?? 'best',
    output: config.output ?? process.cwd(),
    rename: config.rename,
    format: config.format,
    compatible: config.compatible ?? false,
    recode: config.recode ?? false,
    'concurrent-downloads': String(config.concurrentDownloads ?? 2),
    'filename-template': config.filenameTemplate,
    'folder-template': config.folderTemplate,
    'adaptive-concurrency': config.adaptiveConcurrency ?? true,
    'check-space': config.checkSpace ?? true,
    timings: config.timings ?? true,
    color: config.color ?? true,
    'playlist-concurrency': String(config.playlistConcurrency ?? 2),
    cookies: config.cookies,
    'cookies-from-browser': config.cookiesFromBrowser,
    'concurrent-fragments': String(config.concurrentFragments ?? 8),
    'experimental-fragments': config.experimentalFragments ?? false,
    'sub-langs': config.subLangs,
    'sponsorblock-remove': config.sponsorblockRemove,
    section: config.section,
    open: config.open ?? false,
    audio: config.audio ?? false,
    resume: config.resume ?? false,
    incognito: config.incognito ?? false,
    'neutral-filename': config.neutralFilename ?? false,
    playlist: config.playlist ?? false,
    subs: config.subs ?? false,
    'auto-subs': config.autoSubs ?? false,
    'sub-format': config.subFormat,
    verify: config.verify ?? false,
    'embed-subs': config.embedSubs ?? false,
    'embed-metadata': config.embedMetadata ?? false,
    'embed-thumbnail': config.embedThumbnail ?? false,
    'closest-quality': config.closestQuality ?? false,
    json: config.json ?? false,
    'skip-existing': config.skipExisting ?? false,
    'playlist-items': config.playlistItems,
    source: config.source,
    timeout: config.timeout,
    'deep-scan': config.deepScan ?? false,
    'list-sources': config.listSources ?? false,
    'auto-list-sources': config.autoListSources ?? true,
  };
}

export function cliOptions(config = {}) {
  const defaults = optionDefaults(config);
  const options = {};
  for (const [name, extra] of Object.entries(STRING_OPTIONS)) {
    options[name] = { type: 'string', ...extra };
    if (defaults[name] !== undefined) options[name].default = defaults[name];
  }
  for (const [name, extra] of Object.entries(BOOLEAN_OPTIONS)) {
    options[name] = { type: 'boolean', ...extra };
    if (defaults[name] !== undefined) options[name].default = defaults[name];
  }
  return options;
}

const COMMAND_FLAGS = {
  stats: ['--json'], history: ['--json', '--limit', '--failed'],
  changes: ['--json', '--version', '--since', '--to', '--range', '--latest'],
  runs: ['--json'], stop: [], flush: ['--stats'],
  inspect: ['--json', '--check-audio', '--search'],
  doctor: ['--offline', '--output', '-o'],
  update: ['--check'], up: ['--check'], upgrade: ['--check'], check: ['--check'],
  backend: ['--check', '--keep-files'],
  alias: ['--json', '--force', '--bin-dir'],
  uninstall: ['--prefix', '-p', '--yes', '--keep-cache', '--keep-config', '--keep-aliases', '--json', '--bin-dir'],
  config: ['--profile', '--external', '--terminal'],
  profile: [],
};
// `changelog` is an alias, so it shares the flags of the command it stands for.
const ALIASES = { changelog: 'changes' };
const COMMAND_NAMES = [...Object.keys(COMMAND_FLAGS), ...Object.keys(ALIASES), 'retry', 'subs', 'help', 'version'];

function mistypedCommandOption(args) {
  const command = args[0];
  if (command && !command.startsWith('-') && !COMMAND_NAMES.includes(command)) {
    const suggestion = suggestOption(command, COMMAND_NAMES);
    if (suggestion) return `Unknown command "${command}". Did you mean "${suggestion}"?`;
  }
  const flags = Object.hasOwn(COMMAND_FLAGS, command) ? COMMAND_FLAGS[command]
    : ALIASES[command] ? COMMAND_FLAGS[ALIASES[command]] : null;
  const options = flags
    ? [...flags, '--help', '-h', '--no-color', '--color', '--profile']
    : command === 'retry' ? [...optionSpellings(cliOptions()), '--last'] : null;
  if (!options) return null; // Download flags are checked by parseCli itself.
  const start = ['config', 'backend', 'alias'].includes(command) && args[1] && !args[1].startsWith('-') ? 2 : 1;
  for (const arg of args.slice(start)) {
    if (arg === '--') break;
    const name = arg.split('=')[0];
    if (!name.startsWith('-') || options.includes(name)) continue;
    const suggestion = suggestOption(name, options);
    if (suggestion) return `Unknown option "${name}". Did you mean "${suggestion}"?`;
  }
  return null;
}

export function parseCli(args, { config = {} } = {}) {
  if (args[0] === 'help') args = ['--help', ...args.slice(1)];
  if (args[0] === 'version') {
    if (args.length !== 1) throw new Error('Usage: veo version');
    return { version: true };
  }
  let preliminary;
  try { preliminary = parseArgs({ args, allowPositionals: true, strict: true, allowNegative: true, options: cliOptions() }); }
  catch (error) { throw explainUnknownOption(error, cliOptions()); }
  const profileName = selectedProfileName(config, preliminary.values.profile);
  config = applyProfile(config, preliminary.values.profile);
  let parsed;
  try { parsed = parseArgs({ args, tokens: true, allowNegative: true, allowPositionals: true, strict: true, options: cliOptions(config) }); }
  catch (error) { throw explainUnknownOption(error, cliOptions(config)); }
  const { values, positionals, tokens } = parsed;
  if (values.help || values.version) return values;
  // A stored default conflicting with a flag typed right now is a user error;
  // a stored default merely ignored by another flag is not.
  const typed = {
    quality: tokens.some(token => token.name === 'quality'),
    closest: tokens.some(token => token.name === 'closest-quality'),
    audio: tokens.some(token => ['audio', 'no-audio'].includes(token.name)),
  };
  // An explicit numeric --quality states video intent, so it overrides an
  // audio-only default instead of turning into a confusing conflict error.
  if (typed.quality && values.quality !== 'best' && values.audio && !typed.audio) values.audio = false;
  if (!positionals.length && !values['batch-file'] && !values['retry-failed']) throw new Error('Provide at least one video URL. Run veo --help for usage.');
  if (!QUALITIES.includes(values.quality) && !/^[1-9]\d{1,4}p$/.test(values.quality)) throw new Error(`Invalid quality. Choose: ${QUALITIES.join(', ')} or a numeric resolution.`);
  if (values['playlist-items']) {
    if (values.playlist === false && tokens.some(token => token.name === 'playlist')) {
      if (tokens.some(token => token.name === 'playlist-items')) throw new Error('--playlist-items cannot be combined with --no-playlist.');
      values['playlist-items'] = undefined;
    } else { validateItems(values['playlist-items']); values.playlist = true; }
  }
  if (values['retry-failed'] && (positionals.length || values['batch-file'])) throw new Error('--retry-failed cannot be combined with URLs or --batch-file.');
  if (values.incognito && values['retry-failed']) throw new Error('--incognito cannot retry a saved job. Use --no-incognito for this request.');
  if (values.incognito && values.resume) {
    if (tokens.some(token => token.name === 'resume')) throw new Error('--incognito cannot be combined with --resume.');
    values.resume = false;
  }
  if (values.incognito && values['skip-existing']) {
    if (tokens.some(token => token.name === 'skip-existing')) throw new Error('--incognito cannot be combined with --skip-existing.');
    values['skip-existing'] = false;
  }
  if (!values.output.trim()) throw new Error('The output directory cannot be empty.');
  if (values.rename !== undefined && !cleanText(values.rename)) throw new Error('The custom filename cannot be empty.');
  if (!values['neutral-filename'] && positionals.length > 1 && values.rename !== undefined && !values.rename.includes('*')) throw new Error('--rename only applies to a single URL unless the name contains * for the original title.');
  if (values.audio && values.quality !== 'best' && typed.quality) throw new Error('--quality is for video; omit it when using --audio.');
  if (values.audio && values['closest-quality'] && typed.closest) throw new Error('--closest-quality is for video; omit it when using --audio.');
  if (values['closest-quality'] && values.quality === 'best' && typed.closest) throw new Error('--closest-quality requires a numeric --quality such as 1080p.');
  values['list-qualities'] = Boolean(values['list-qualities'] || values['list-qualitys']);
  delete values['list-qualitys'];
  if (values['list-qualities']) {
    if (positionals.length !== 1 || values['batch-file'] || values['retry-failed']) throw new Error('--list-qualities accepts exactly one URL.');
    if (values['list-formats'] || values['list-sources'] || values['list-subs']) throw new Error('--list-qualities cannot be combined with other listing actions.');
    if (values.audio || values.playlist) throw new Error('--list-qualities requires a single video without --audio or --playlist.');
  }
  if (values['list-formats'] && (values.audio || values.format)) throw new Error('--list-formats cannot be combined with --audio or --format.');
  if (values['list-formats'] && positionals.length > 1) throw new Error('--list-formats accepts exactly one URL.');
  if (values['list-subs']) {
    if (positionals.length !== 1 || values['batch-file'] || values['retry-failed']) throw new Error('--list-subs accepts exactly one URL.');
    if (values['list-formats'] || values['list-sources']) throw new Error('--list-subs cannot be combined with other listing actions.');
    if (values.playlist) throw new Error('--list-subs requires a single video without --playlist.');
  }
  if (values['sub-format'] && (!/^[a-z0-9]+(?:\/[a-z0-9]+)*$/i.test(values['sub-format']) || values['sub-format'].length > 100)) throw new Error('--sub-format must be a format or preference list such as srt/vtt/best.');
  if (values.source && (!/^[1-9]\d*$/.test(values.source) || !Number.isSafeInteger(Number(values.source)))) throw new Error('--source requires a positive source number.');
  if (values.source && values['list-sources'] && !tokens.some(token => token.name === 'list-sources')) values['list-sources'] = false;
  if (values['list-sources'] && !tokens.some(token => token.name === 'list-sources')
    && (positionals.length !== 1 || values['batch-file'] || values['retry-failed'])) values['list-sources'] = false;
  if (values['list-sources'] && (positionals.length !== 1 || values['batch-file'] || values['retry-failed'])) throw new Error('--list-sources accepts exactly one URL.');
  if (values.source && (positionals.length > 1 || values['batch-file'])) throw new Error('--source accepts exactly one URL.');
  if (values.source && values['list-sources']) throw new Error('--source and --list-sources are separate actions.');
  if (values.timeout) parseSourceTimeout(values.timeout);
  if ((values.source || values['list-sources']) && !tokens.some(token => ['playlist', 'playlist-items'].includes(token.name))) {
    values.playlist = false;
    values['playlist-items'] = undefined;
  }
  if ((values.source || values['list-sources']) && values.playlist) throw new Error('--source and --list-sources cannot be combined with --playlist.');
  if (values['concurrent-fragments'] !== undefined) {
    const fragments = Number(values['concurrent-fragments']);
    validateFragments(fragments);
  }
  const concurrency = Number(values['playlist-concurrency']);
  if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > 4) throw new Error('--playlist-concurrency must be a whole number between 1 and 4.');
  if (values.compatible && (values.audio || (values.format && values.format !== 'mp4') || values.recode || values['embed-subs'] || values['embed-thumbnail'])) throw new Error('--compatible requires video MP4 without --recode, --embed-subs or --embed-thumbnail.');
  if (values.compatible) values.format = 'mp4';
  if (values.recode && (!values.format || values.audio)) throw new Error('--recode requires --format and video mode.');
  if (values.section && !/^[*\d]/.test(values.section.trim())) throw new Error('--section requires a range such as "*10:00-12:00" or "10:00-12:00".');
  if (values['sponsorblock-remove'] && !/^[a-z_,-]+$/i.test(values['sponsorblock-remove'].trim())) throw new Error('--sponsorblock-remove takes comma-separated category names, e.g. "sponsor,selfpromo".');
  const formats = values.audio ? AUDIO_FORMATS : VIDEO_FORMATS;
  if (values.format && !formats.includes(values.format)) throw new Error(`Invalid ${values.audio ? 'audio' : 'video'} format. Choose: ${formats.join(', ')}.${!values.audio && AUDIO_FORMATS.includes(values.format) ? ' Use --audio for audio formats.' : ''}`);

  const options = {};
  for (const [name, value] of Object.entries(values)) {
    if (value === undefined) continue;
    options[name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  options.urls = positionals.map(validateUrl);
  options.url = options.urls[0];
  if (values.cookies !== undefined) options.cookies = validateCookieFile(values.cookies);
  if (values['cookies-from-browser'] !== undefined) options.cookiesFromBrowser = validateBrowserSpec(values['cookies-from-browser']);
  if (options.section) options.section = options.section.trim();
  if (options.sponsorblockRemove) options.sponsorblockRemove = options.sponsorblockRemove.trim();
  if (options.concurrentFragments !== undefined) options.concurrentFragments = Number(options.concurrentFragments);
  if (options.timeout) options.timeoutMs = parseSourceTimeout(options.timeout);
  options.playlistConcurrency = concurrency;
  options.concurrentDownloads = Number(values['concurrent-downloads']);
  if (!Number.isInteger(options.concurrentDownloads) || options.concurrentDownloads < 1 || options.concurrentDownloads > 4) throw new Error('--concurrent-downloads must be a whole number between 1 and 4.');
  if (options.filenameTemplate !== undefined) validateTemplate(options.filenameTemplate);
  if (options.folderTemplate !== undefined) validateTemplate(options.folderTemplate, { folders: true });
  if (options.rename && options.filenameTemplate && !options.neutralFilename) throw new Error('--rename and --filename-template cannot be combined.');
  const disableSubs = values.subs === false && tokens.some(token => token.name === 'subs');
  if (options.subLangs && !disableSubs && !options.autoSubs) options.subs = true;
  if (disableSubs) {
    options.embedSubs = false;
    if (!options.autoSubs) options.subLangs = undefined;
  }
  options.profile = profileName;
  return options;
}

export async function main(args = process.argv.slice(2), { config } = {}) {
  if (args[0] === 'help') args = ['--help', ...args.slice(1)];
  const mistyped = mistypedCommandOption(args);
  if (mistyped) { process.stderr.write(`veo: ${mistyped}\n`); return 1; }
  let color = !args.includes('--no-color') && !args.includes('--json');
  let display;
  let loadedConfig;
  try {
    display = outputOptions(args);
    const loaded = config ?? await loadConfig();
    loadedConfig = loaded.config || {};
    color = !args.includes('--json') && (display.color ?? applyProfile(loaded.config || {}, display.profile).color ?? true);
    // Validate explicit profiles even when a color flag overrides their setting.
    if (display.profile) applyProfile(loaded.config || {}, display.profile);
  } catch (error) {
    if (!display || display.profile) {
      process.stderr.write(`veo: ${readableError(error)}\n`);
      return 1;
    }
    // The command's own validation reports configuration errors.
  }
  if (['stats', 'history', 'changes', 'changelog', 'flush', 'runs', 'inspect', 'stop', 'update', 'up', 'upgrade', 'check', 'doctor', 'backend', 'alias', 'uninstall'].includes(args[0])) args = display.remaining;
  const code = await withOutputSettings(color, () => runMain(args, { config }));
  // One throttled (once per day) update hint after every successful command.
  // Failures, cancellations, help/version output and the update commands
  // themselves never trigger it; VEO_NO_UPDATE_CHECK=1 disables it for scripts.
  let incognitoRequest = !args.length;
  if (!incognitoRequest && code === 0 && shouldUpdateNotice(args)) {
    try { incognitoRequest = Boolean(parseCli(args, { config: loadedConfig }).incognito); }
    catch { /* Non-download commands have their own argument parser. */ }
  }
  if (code === 0 && shouldUpdateNotice(args) && !incognitoRequest) {
    try {
      const notice = await maybeUpdateNotice({ currentVersion: await packageVersion() });
      if (notice) process.stderr.write(`${notice}\n`);
    } catch {
      // Informational only: a broken notice must never fail the command.
    }
  }
  return code;
}

// Help, version and the update commands report versions themselves;
// anything else earns the daily update hint on success.
export function shouldUpdateNotice(args = []) {
  if (args.includes('-h') || args.includes('--help') || args.includes('-v') || args.includes('--version')) return false;
  if (['update', 'up', 'upgrade', 'check', 'version', 'help', 'changes', 'changelog'].includes(args[0])) return false;
  return true;
}

async function runMain(args, { config }) {
  const stdout = outputStream(process.stdout, { plain: args.includes('--json') });
  const stderr = outputStream(process.stderr, { plain: args.includes('--json') });
  if (args[0] === 'stats') {
    try { return await (await import('./stats.js')).statsMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'history') {
    try { return await (await import('./history.js')).historyMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'changes' || args[0] === 'changelog') {
    try { return await (await import('./changes.js')).changesMain(args.slice(1), { stdout }); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'flush') {
    try { return await (await import('./flush.js')).flushMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'runs') {
    try { return await (await import('./runs.js')).runsMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'inspect') {
    try { return await (await import('./inspect-media.js')).inspectMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return error.name === 'AbortError' ? 130 : 1; }
  }
  if (args[0] === 'stop') {
    try { return await (await import('./runs.js')).stopMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'retry') {
    if (args[1] !== '--last') {
      stderr.write('veo: Usage: veo retry --last [download options]\n');
      return 1;
    }
    try {
      const { latestFailedJob } = await import('./jobs.js');
      args = ['--retry-failed', await latestFailedJob(), ...args.slice(2)];
    } catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'subs') {
    if (args[1] === '--help' || args[1] === '-h') {
      stdout.write('veo subs <url> [--json] [--source <n>] [--profile <name>]\n\nList manual and automatically generated subtitle languages and source formats without downloading media.\n');
      return 0;
    }
    if (!args[1] || args[1].startsWith('-')) {
      stderr.write('veo: Usage: veo subs <url> [--json] [--source <n>]\n');
      return 1;
    }
    args = [args[1], '--list-subs', ...args.slice(2)];
  }
  if (args[0] === 'profile') {
    try { return await profileMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  const { cleanupDownloadCache } = await import('./download-cache.js');
  await cleanupDownloadCache().catch(error => stderr.write(`veo: Could not clean expired local downloads: ${readableError(error)}\n`));
  // update/upgrade/up/check subcommands are handled before URL validation.
  if (['update', 'up', 'upgrade', 'check'].includes(args[0])) {
    if (args[0] === 'up') args = ['update', ...args.slice(1)];
    if (args[0] === 'check' && args[1] !== 'update') {
      stdout.write(UPDATE_HELP);
      return 0;
    }
    return updateMain(args, { registry: defaultRegistry() });
  }
  if (args[0] === 'config') {
    try { return await configMain(args.slice(1)); }
    catch (error) { stderr.write(`veo: ${readableError(error)}\n`); return 1; }
  }
  if (args[0] === 'doctor') {
    try {
      const { doctorMain } = await import('./doctor.js');
      return await doctorMain(args.slice(1));
    } catch (error) {
      stderr.write(`veo: ${readableError(error)}\n`);
      return 1;
    }
  }
  if (args[0] === 'backend') {
    try {
      const { backendUpdateMain } = await import('./backend-update.js');
      return await backendUpdateMain(args.slice(1));
    } catch (error) {
      stderr.write(`veo: ${readableError(error)}\n`);
      return 1;
    }
  }
  if (args[0] === 'alias') {
    try {
      const { aliasMain } = await import('./alias.js');
      return await aliasMain(args.slice(1));
    } catch (error) {
      stderr.write(`veo: ${readableError(error)}\n`);
      return 1;
    }
  }
  if (args[0] === 'uninstall') {
    try {
      const { uninstallMain } = await import('./uninstall.js');
      return await uninstallMain(args.slice(1));
    } catch (error) {
      stderr.write(`veo: ${readableError(error)}\n`);
      return 1;
    }
  }
  const reporter = createReporter();
  const controller = new AbortController();
  const cancel = () => controller.abort();
  let run;
  process.once('SIGINT', cancel);
  process.once('SIGTERM', cancel);
  try {
    const loaded = config ?? await loadConfig();
    for (const warning of loaded.warnings || []) stderr.write(`veo: ${warning}\n`);
    if (!args.length && process.stdin.isTTY && process.stderr.isTTY) {
      const { interactiveArgs } = await import('./interactive.js');
      args = await interactiveArgs(loaded.config || {}, { signal: controller.signal });
      if (!args) return 0;
    }
    let options = parseCli(args, { config: loaded.config });
    if (options.help) { stdout.write(HELP); return 0; }
    if (options.version) {
      const pkg = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
      stdout.write(`${pkg.version}\n`);
      return 0;
    }
    if (!options.incognito) run = await (await import('./runs.js')).registerRun(cancel);
    let retryItems;
    if (options.retryFailed) {
      retryItems = await retryOptions(options.retryFailed);
      // Revalidate stored options and apply only flags explicitly supplied now.
      const retryArgs = args.filter((arg, index) => arg !== '--retry-failed' && args[index - 1] !== '--retry-failed' && !arg.startsWith('--retry-failed='));
      const overrideProfile = retryArgs.some(arg => arg === '--profile' || arg.startsWith('--profile='));
      retryItems = retryItems.map(item => {
        const resumed = parseCli([item.url, ...retryArgs], { config: overrideProfile
          ? { ...item, profiles: loaded.config?.profiles }
          : item });
        if (!overrideProfile) resumed.profile = item.profile;
        return resumed;
      });
      options = { ...retryItems[0], urls: retryItems.map(item => item.url) };
    }
    if (options.batchFile) {
      const lines = (await readFile(options.batchFile, 'utf8')).replace(/^\uFEFF/, '').split(/\r?\n/).map(line => line.trim()).filter(line => line && !line.startsWith('#'));
      options.urls.push(...lines.map(validateUrl));
      options.url = options.urls[0];
      if (!options.urls.length) throw new Error('The URL list is empty.');
      if (!options.neutralFilename && options.rename && options.urls.length > 1 && !options.rename.includes('*')) throw new Error('--rename only applies to a single URL unless the name contains * for the original title.');
      if (options.listFormats && options.urls.length > 1) throw new Error('--list-formats accepts exactly one URL.');
    }
    await confirmExperimentalFragments(retryItems || [options], { signal: controller.signal });
    // The job file is created before the first download, so another terminal can
    // follow this run's per-item progress with `veo runs <id>`.
    const jobFile = options.incognito || options.listQualities || options.listFormats || options.listSources || options.listSubs || options.dryRun ? null : jobFilePath();
    await run?.describe({ urls: options.urls, output: options.output ? path.resolve(options.output) : null,
      audio: options.audio, quality: options.quality, format: options.format, profile: options.profile || null,
      playlist: options.playlist, job: jobFile });
    reporter.configure?.({ color: options.color && !options.json });
    reporter.start(options.rename);
    if (!options.dryRun && !options.listQualities && !options.listFormats && !options.listSources && !options.listSubs) reporter.profile(options.profile);
    const preparedBackends = new Map();
    const reuseBackend = request => preparedBackends.has(request.url) ? async () => preparedBackends.get(request.url) : undefined;
    const cookieWarning = cookieFileWarning(options.cookies);
    if (cookieWarning) stderr.write(`veo: ${cookieWarning}\n`);

    // For a single page, fall back to media requests observed while its player loads.
    // Explicit --source rediscovers unless a completed local transfer is usable.
    if (!retryItems && options.urls.length === 1 && (!options.playlist || options.listSources)) {
      const discovery = await resolvePageSource(options, { signal: controller.signal, stderr, reporter,
        onBackend: backend => preparedBackends.set(options.url, backend) });
      if (options.listSources) {
        if (options.json) stdout.write(`${JSON.stringify({ url: options.url, status: 'sources', timedOut: Boolean(discovery.sources.timedOut), sources: discovery.sources.map(({ url, pageUrl, ...safe }) => safe) })}\n`);
        else for (const source of discovery.sources) stdout.write(`${discovery.formatSource(source)}\n`);
        if (discovery.sources.timedOut) stderr.write(`veo: Source search timed out after ${options.timeout || (options.deepScan ? '120s' : '45s')}; showing verified sources found so far.\n`);
        if (!discovery.sources.length) stderr.write(discovery.sources.timedOut
          ? 'veo: No sources were verified before the timeout.\n'
          : 'veo: No downloadable sources were found on this page.\n');
        return discovery.sources.length && !discovery.sources.timedOut ? 0 : 1;
      }
      if (discovery.needsSelection) {
        if (options.json) stdout.write(`${JSON.stringify({ url: options.url, status: 'sources', timedOut: Boolean(discovery.sources.timedOut), sources: discovery.sources.map(({ url, pageUrl, ...safe }) => safe) })}\n`);
        else for (const source of discovery.sources) stdout.write(`${discovery.formatSource(source)}\n`);
        stderr.write('veo: Select a source with --source <number>.\n');
        return 1;
      }
      if (discovery.mediaUrl) {
        options.mediaUrl = discovery.mediaUrl;
        options.source = String(discovery.source);
      }
    }
    if (retryItems) for (const item of retryItems) {
      if (!item.source) continue;
      const discovery = await resolvePageSource(item, { signal: controller.signal, stderr, reporter,
        onBackend: backend => preparedBackends.set(item.url, backend) });
      item.mediaUrl = discovery.mediaUrl;
    }

    if (options.listSubs) {
      const { listSubtitles } = await import('./downloader.js');
      const result = await listSubtitles(options, { signal: controller.signal, reporter, backendResolver: reuseBackend(options) });
      if (options.json) stdout.write(`${JSON.stringify(result)}\n`);
      else {
        stdout.write(`Subtitles: ${cleanText(result.title || options.url)}\n`);
        for (const [label, entries] of [['Manual', result.manual], ['Automatic', result.automatic]]) {
          stdout.write(`${label}:\n`);
          if (!entries.length) stdout.write('  none\n');
          for (const entry of entries) stdout.write(`  ${cleanText(entry.language)}${entry.name ? ` (${cleanText(entry.name)})` : ''}: ${entry.formats.join(', ') || 'unknown'}\n`);
        }
      }
      return 0;
    }
    if (options.listQualities) {
      const { listQualities } = await import('./downloader.js');
      const result = await listQualities(options, { signal: controller.signal, reporter, backendResolver: reuseBackend(options) });
      if (options.json) stdout.write(JSON.stringify(result) + '\n');
      else {
        stdout.write('Available qualities: ' + (result.qualities.join(', ') || 'resolution unknown') + '\n');
        if (result.qualities.length) stdout.write('Choose with --quality ' + result.qualities[0] + '\n');
      }
      return 0;
    }
    if (options.listFormats) {
      const { listFormats } = await import('./downloader.js');
      stdout.write(await listFormats(options, { signal: controller.signal, reporter, backendResolver: reuseBackend(options) }));
      return 0;
    }
    if (options.dryRun) {
      const { planDownload } = await import('./downloader.js');
      for (const request of retryItems || options.urls.map(url => ({ ...options, url }))) {
        const { url } = request;
        const plan = await planDownload(request, { signal: controller.signal, reporter, backendResolver: reuseBackend(request) });
        if (options.json) stdout.write(`${JSON.stringify({ url, status: 'planned', ...plan })}\n`);
        else {
          stdout.write(`URL: ${url}\n`);
          stdout.write(`${describeEstimate(plan)}\n`);
          if (plan.quality) stdout.write(`${plan.quality}\n`);
          for (const entry of plan.entries) stdout.write(`Would save: ${cleanText(entry.path)}\n`);
        }
      }
      return 0;
    }
    const { download } = await import('./downloader.js');
    const { createStatsRecorder } = await import('./stats.js');
    const { createHistoryRecorder } = await import('./history.js');
    if (!options.json && process.stdout.isTTY) reporter.enableInline();
    const result = await runJob(options, { download: (request, dependencies) => download(request, { ...dependencies, backendResolver: reuseBackend(request) }),
      reporter, stdout: reporter.output(stdout), stderr: reporter.output(stderr), signal: controller.signal, openFile, items: retryItems, jobFile: jobFile || undefined, runId: run?.id,
      recordStats: options.incognito ? undefined : createStatsRecorder(), recordHistory: options.incognito ? undefined : createHistoryRecorder() });
    return result;
  } catch (error) {
    reporter.fail(controller.signal.aborted);
    reporter.dispose();
    stderr.write(`veo: ${readableError(error)}\n`);
    return controller.signal.aborted || error.name === 'AbortError' ? 130 : 1;
  } finally {
    reporter.dispose();
    await run?.unregister();
    process.removeListener('SIGINT', cancel);
    process.removeListener('SIGTERM', cancel);
  }
}

async function askSourceQuestion(prompt, signal) {
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  try { return await rl.question(prompt, { signal }); }
  finally { rl.close(); }
}

export async function resolvePageSource(options, { signal, stderr, reporter, onBackend, inspect, discover, ask = askSourceQuestion,
  backendResolver, localRoot, interactive = Boolean(process.stdin.isTTY && process.stderr.isTTY && !options.json) }) {
  const { fetchMetadata, prepareBackend, readReadyTransfer, runBackend } = await import('./downloader.js');
  // Source validation remains necessary for new downloads and metadata actions;
  // a completed local transfer can finish while its original source is offline.
  if (!options.incognito && !options.dryRun && !options.listSources && !options.listFormats
    && !options.listQualities && !options.listSubs && await readReadyTransfer(options, { localRoot, signal })) return {};
  const { discoverSources, formatSource, shouldOfferSourceDiscovery } = await import('./source-discovery.js');
  const backend = inspect ? null : await (backendResolver || prepareBackend)(options, { signal, reporter });
  if (backend) onBackend?.(backend);
  const inspectMedia = (request, requestSignal = signal) => inspect ? inspect(request, requestSignal) : fetchMetadata(request, { signal: requestSignal, backend, runner: runBackend });
  let originalError;
  if (!options.source && !options.listSources) {
    try {
      const metadata = await inspectMedia(options);
      if (!metadata.entries?.length && !metadata.formats?.length && !metadata.url) {
        throw new Error('No downloadable video formats found.');
      }
      return {};
    }
    catch (error) { originalError = error; }
    if (!shouldOfferSourceDiscovery(originalError)) throw originalError;
    if (options.autoListSources === false) throw new Error(`${originalError.message} Use --list-sources to search this page, then --source <number> to select one.`);
    stderr.write('veo: No direct video found; searching the page for a playable source…\n');
  }
  let sources;
  try {
    sources = await (discover || discoverSources)(options.url, {
      signal, deepScan: options.deepScan, timeoutMs: options.timeoutMs,
      inspect: (url, _page, { signal: scanSignal } = {}) => inspectMedia({ ...options, mediaUrl: url }, scanSignal),
    });
  } catch (error) {
    if (signal?.aborted || error?.name === 'AbortError') throw error;
    if (originalError) throw new Error(`${originalError.message} Source discovery also failed: ${error.message}`);
    throw error;
  }
  if (options.listSources) return { sources, formatSource };
  if (sources.timedOut) stderr.write(`veo: Source search timed out after ${options.timeout || (options.deepScan ? '120s' : '45s')}; showing verified sources found so far.\n`);
  if (!sources.length) {
    if (originalError) throw originalError;
    throw new Error('No downloadable video sources were found on this page.');
  }
  let index = Number(options.source);
  if (!index) {
    if (sources.length === 1) index = 1;
    else if (!interactive) return { sources, formatSource, needsSelection: true };
  }
  if (!index) {
    for (const source of sources) stderr.write(`${formatSource(source)}\n`);
    const answer = await ask('Source number (Enter to cancel): ', signal);
    if (!/^[1-9]\d*$/.test(answer.trim())) throw new Error('No source selected.');
    index = Number(answer.trim());
  }
  const selected = sources[index - 1];
  if (!selected) throw new Error(`Source ${index} does not exist; choose 1-${sources.length}.`);
  return { mediaUrl: selected.url, source: index };
}
