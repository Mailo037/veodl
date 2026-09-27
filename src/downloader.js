import { ensureCompatibility } from './compatibility.js';
import { adaptiveRun, reducedLimit, phaseTimer } from './execution.js';
import { mediaDestination, prepareDestination } from './naming.js';
import { estimateMediaBytes, reserveSpace } from './disk-space.js';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { lstat, mkdir, open, readdir, rm, rmdir, stat } from 'node:fs/promises';
import path from 'node:path';
import { resolveBackend } from './backend.js';
import { availableHeights, cappedHeight, closestHeight, saveUnique, sanitizeTitle } from './utils.js';
import { digest, readJson, writeJson } from './state.js';
import { cleanupDownloadCache, downloadCacheRoot, TRANSFER_RETENTION_MS } from './download-cache.js';
import { selectedEntries, sizeEstimate, describeEstimate } from './playlist.js';

const PARTIAL_PREFIX = '.veo-part-';
const STAGED_MEDIA = /^media(?:-(\d+))?\.([A-Za-z0-9]{1,8})$/;
// Only real media extensions may count as the downloaded file, so a thumbnail
// or a subtitle that the backend leaves behind is never mistaken for it.
const MEDIA_EXTENSIONS = new Set(['mp4', 'mkv', 'webm', 'mov', 'avi', 'm4v', '3gp', 'ts', 'flv', 'mpg', 'mpeg', 'ogv',
  'mp3', 'm4a', 'aac', 'opus', 'flac', 'wav', 'ogg', 'oga', 'wma', 'mka', 'aiff']);

export function isStagedMedia(name) {
  const match = STAGED_MEDIA.exec(name);
  return Boolean(match) && MEDIA_EXTENSIONS.has(match[2].toLowerCase());
}

export function runBackend(executable, args, { signal, onLine } = {}) {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    const child = spawn(executable, args, { shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], signal: process.platform === 'win32' ? undefined : signal });
    const cancelTree = () => {
      if (!child.pid || child.exitCode !== null) return;
      const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { shell: false, windowsHide: true, stdio: 'ignore' });
      killer.on('error', () => child.kill());
    };
    if (process.platform === 'win32') signal?.addEventListener('abort', cancelTree, { once: true });
    let output = '';
    let errors = '';
    let failure;
    const lines = createInterface({ input: child.stdout });
    const errorLines = createInterface({ input: child.stderr });
    errorLines.on('line', line => {
      if (onLine && /^veo-(?:progress|postprocess):/.test(line)) {
        try { onLine(line); } catch (error) { failure = error; child.kill(); }
      }
    });
    lines.on('line', line => {
      if (onLine) {
        try { onLine(line); } catch (error) { failure = error; child.kill(); }
      } else if (output.length < 32 * 1024 * 1024) output += `${line}\n`;
      else { failure = new Error('Video metadata exceeded the supported size.'); child.kill(); }
    });
    child.stderr.on('data', data => { errors = (errors + data).slice(-16000); });
    child.on('error', error => { failure = error; });
    child.on('close', code => {
      signal?.removeEventListener('abort', cancelTree);
      lines.close();
      errorLines.close();
      if (failure) reject(failure);
      else if (signal?.aborted) reject(new DOMException('Cancelled', 'AbortError'));
      else if (code !== 0) {
        const diagnostics = errors.split(/\r?\n/).filter(line => !/^veo-(?:progress|postprocess):/.test(line));
        const failureLine = diagnostics.filter(line => /^ERROR:/i.test(line)).at(-1);
        reject(new Error(failureLine || diagnostics.join('\n') || `Downloading backend exited with code ${code}.`));
      }
      else resolve(output);
    });
  });
}

export function formatSelector(height) {
  const filter = height ? `[height=${height}]` : '';
  return `bv${filter}+ba/b${filter}/bv${filter}`;
}

/**
 * Turn a requested quality into backend arguments and a user-facing label.
 *
 * Default (upper bound): the best resolution at or below the request, so
 * `-q 720p` on a phone plan can never silently fetch 2160p. A source that
 * offers nothing at or below the request fails before anything is downloaded,
 * naming the resolutions that do exist.
 * `--closest-quality` keeps the historical "nearest available height" rule.
 * Sources without resolution metadata fall back to the best available stream,
 * and collections use the backend's own resolution preference per entry.
 */
export function selectQuality({ quality = 'best', audio = false, closest = false, formats, playlist = false } = {}) {
  if (audio || quality === 'best') return { format: null, sort: null, label: null };
  const target = Number.parseInt(quality, 10);
  if (playlist) {
    // Entries differ in resolution, so cap them inside the backend instead.
    return {
      format: null,
      sort: `res:${target}`,
      label: closest ? `Quality: closest to ${target}p per entry` : `Quality: up to ${target}p per entry`,
    };
  }
  const heights = availableHeights(formats);
  if (!heights.length) return { format: null, sort: null, label: 'Resolution unknown; using the best available stream.' };
  if (closest) {
    const height = closestHeight(formats, quality);
    return {
      format: formatSelector(height),
      sort: null,
      label: `Quality: ${height}p${height === target ? '' : ` (closest to ${quality})`}`,
    };
  }
  const height = cappedHeight(formats, quality);
  if (!height) {
    const offered = [...heights].sort((a, b) => a - b).map(value => `${value}p`).join(', ');
    return {
      format: null,
      sort: null,
      label: null,
      error: `No stream at or below ${target}p is available (offered: ${offered}). Use a higher --quality or "best".`,
    };
  }
  return {
    format: formatSelector(height),
    sort: null,
    label: `Quality: ${height}p${height === target ? '' : ` (highest at or below ${quality})`}`,
  };
}

/**
 * A stable, filesystem-safe key for one video, so a re-run with --resume finds
 * the same staging directory. Site ids are untrusted input and never used raw.
 */
export function partialKey(metadata, url, options = {}) {
  return digest({ source: sourceKey(metadata, url), settings: downloadSettings(options) });
}

function sourceKey(metadata, url) {
  return metadata.id && (metadata.extractor_key || metadata.extractor)
    ? `${metadata.extractor_key || metadata.extractor}:${metadata.id}` : `${url}#${metadata.id || ''}`;
}

// A history record can only skip a download while every file it names still
// exists as a plain file inside the same output directory.
export async function historyRecordUsable(record, directory) {
  return Boolean(record?.files?.length && record.files.every(file => typeof file === 'string' && path.dirname(file) === directory)
    && (await Promise.all(record.files.map(file => stat(file).then(info => info.isFile(), () => false)))).every(Boolean));
}

// Drops a duplicate-detection record whose files are gone and removes the
// history folder with its last record, so deleting media leaves no stale
// state behind. Returns whether the record is still usable.
export async function pruneHistoryRecord(historyFile, directory, history) {
  const usable = await historyRecordUsable(history, directory);
  if (history && !usable) await rm(historyFile, { force: true }).catch(() => {});
  if (!usable) await rmdir(path.dirname(historyFile)).catch(() => {});
  return usable;
}

function downloadSettings(options) {
  options = { quality: 'best', audio: false, closestQuality: false, subs: false, embedSubs: false,
    embedMetadata: false, embedThumbnail: false, ...options };
  const settings = Object.fromEntries(['quality', 'audio', 'format', 'closestQuality', 'subs', 'subLangs', 'embedSubs',
    'embedMetadata', 'embedThumbnail', 'sponsorblockRemove', 'section'].map(key => [key, options[key] ?? null]));
  // Existing conversion jobs retain their keys; remux requests must not reuse them.
  if (options.format && !options.audio && !options.recode) settings.videoRemux = true;
  if (options.compatible) settings.compatible = true;
  if (options.filenameTemplate) settings.filenameTemplate = options.filenameTemplate;
  if (options.folderTemplate) settings.folderTemplate = options.folderTemplate;
  if (options.source) settings.source = options.source;
  return settings;
}

export function localRequestKey(options) {
  return digest({ url: options.url, entry: options._entryIndex || null, settings: downloadSettings(options), output: path.resolve(options.output) });
}

/**
 * Resuming reuses a predictable directory name, so it must not follow a
 * symlink or a file that another process placed there.
 */
async function prepareStaging(partials) {
  try {
    await mkdir(partials);
  } catch (error) {
    if (error.code !== 'EEXIST') throw error;
  }
  const info = await lstat(partials).catch(() => undefined);
  if (!info?.isDirectory() || info.isSymbolicLink()) throw new Error(`The partial download path is not a plain directory: ${partials}`);
  return partials;
}

/**
 * A finished download whose save step failed earlier is already complete; using
 * it avoids relying on the backend's "--no-overwrites" skip behaviour.
 * Returns the largest staged media file, ignoring fragments and .part files.
 */
export async function findFinishedMedia(directory) {
  const entries = await readdir(directory, { withFileTypes: true }).catch(() => []);
  const candidates = [];
  for (const entry of entries) {
    if (!entry.isFile() || !isStagedMedia(entry.name)) continue;
    const file = path.join(directory, entry.name);
    candidates.push({ file, size: (await stat(file).catch(() => ({ size: 0 }))).size });
  }
  return candidates.sort((a, b) => b.size - a.size)[0]?.file;
}

// Formats the backend may leave beside the media file. An allowlist keeps
// merge fragments such as media.f137.mp4 out, since they are never deliverables.
const SIDECAR_EXTENSIONS = new Set(['vtt', 'srt', 'ass', 'lrc', 'ttml', 'srv1', 'srv2', 'srv3', 'json', 'jpg', 'jpeg', 'png', 'webp']);

/** Files a download may leave beside the media file (subtitles, thumbnails). */
export function isSidecar(name) {
  if (name.startsWith('.') || isStagedMedia(name)) return false;
  return SIDECAR_EXTENSIONS.has(path.extname(name).slice(1).toLowerCase());
}

/**
 * Title of one staged file. An explicit --rename wins, and collections use the
 * matching entry title so every entry keeps its own name.
 */
export function stagedTitle(file, metadata, rename) {
  const index = isStagedMedia(path.basename(file)) ? STAGED_MEDIA.exec(path.basename(file))[1] : undefined;
  const fallback = metadata?.title || metadata?.id || 'video';
  const title = index ? metadata?.entries?.[Number(index) - 1]?.title || `${fallback} - ${index}` : fallback;
  if (rename?.includes('*')) return rename.replaceAll('*', () => title);
  if (rename) return index ? `${rename} - ${index}` : rename;
  return title;
}

function backendArgs(options, backend) {
  const common = ['--ignore-config', '--no-plugin-dirs', '--no-colors', '--no-warnings',
    '--socket-timeout', '30', '--retries', '3', '--fragment-retries', '3',
    '--no-js-runtimes', '--js-runtimes', `node:${process.execPath}`,
    '--ffmpeg-location', backend.ffmpegLocation];
  if (!options.playlist) common.push('--no-playlist');
  if (options._entryIndex) common.push('--playlist-items', String(options._entryIndex));
  common.push('--concurrent-fragments', String(options.concurrentFragments ?? 8));
  // Credentials are opt-in per invocation and only ever forwarded to the backend,
  // which never bypasses access controls on its own.
  if (options.cookies) common.push('--cookies', options.cookies);
  if (options.cookiesFromBrowser) common.push('--cookies-from-browser', options.cookiesFromBrowser);
  if (options.url) {
    try {
      const referer = options.mediaUrl ? options.url : `${new URL(options.url).origin}/`;
      common.push('--referer', referer);
    } catch {}
  }
  return common;
}

function mediaArgs(options, quality) {
  const args = ['--no-overwrites', options.resume ? '--continue' : '--no-continue', '--newline',
    '--progress', '--progress-delta', '0.2',
    '--progress-template', 'download:veo-progress:{"progress":%(progress)j,"video":%(info.vcodec|null)j,"audio":%(info.acodec|null)j}',
    '--progress-template', 'postprocess:veo-postprocess:%(progress)j',
    '--print', 'after_move:veo-file:%(filepath)j', '--no-simulate'];
  if (options.audio) args.push('-f', 'ba/b', '--extract-audio', '--audio-format', options.format || 'mp3', '--audio-quality', '0');
  else {
    if (quality.format) args.push('-f', quality.format);
    // Merge into a permissive container first; e.g. H.264 cannot be merged
    // straight into WebM before the requested codec conversion runs.
    if (options.compatible) args.push('--merge-output-format', 'mkv');
    else if (options.format) args.push('--merge-output-format', 'mkv', options.recode ? '--recode-video' : '--remux-video', options.format);
    else args.push('--merge-output-format', 'mp4/mkv');
  }
  const sort = ['vcodec:h264,acodec:aac', quality.sort].filter(Boolean).join(',');
  // Prefer broadly playable sources for MP4/MKV too. Explicit recoding and
  // WebM keep their own source selection. Resolution remains the primary choice.
  const selectedSort = options.recode || options.format === 'webm' ? quality.sort : sort;
  if (!options.audio && selectedSort) args.push('-S', selectedSort);
  if (options.subs || options.subLangs || options.embedSubs) {
    args.push('--write-subs', '--sub-langs', options.subLangs || 'en.*,en');
  }
  if (options.embedSubs) args.push('--embed-subs');
  if (options.embedMetadata) args.push('--embed-metadata');
  if (options.embedThumbnail) args.push('--embed-thumbnail');
  if (options.sponsorblockRemove) args.push('--sponsorblock-remove', options.sponsorblockRemove);
  if (options.section) args.push('--download-sections', options.section, '--force-keyframes-at-cuts');
  return args;
}

export function stagingTemplate(directory, playlist) {
  const name = playlist ? 'media-%(playlist_index)03d.%(ext)s' : 'media.%(ext)s';
  return path.join(directory, name).replaceAll('%', '%%').replace('%%(playlist_index)03d', '%(playlist_index)03d').replace('%%(ext)s', '%(ext)s');
}

/** Read video metadata once, with the same vetted arguments as a download. */
export async function fetchMetadata(options, { signal, backend, runner, reporter } = {}) {
  const common = backendArgs(options, backend);
  reporter?.status('Reading video…');
  const args = [...common, '--dump-single-json', '--skip-download'];
  if (options.playlist && !options._entryIndex) args.push('--flat-playlist');
  args.push('--', options.mediaUrl || options.url);
  try {
    let metadata = JSON.parse(await runner(backend.ytDlp, args, { signal }));
    if (options._entryIndex && metadata.entries) metadata = metadata.entries.find(Boolean);
    if (!metadata) throw new Error('The selected playlist entry is unavailable.');
    if (!options.playlist && (metadata._type === 'playlist' || metadata.entries)) {
      throw new Error('This URL is a collection. Add --playlist to download every entry.');
    }
    if (metadata.is_live) throw new Error('Live streams are not supported. Please use a finished video.');
    if (metadata.has_drm) throw new Error('This content is DRM-protected.');
    reporter?.finishStatus?.('Reading video…', 'done');
    return metadata;
  } catch (error) {
    if (!signal?.aborted && error?.name !== 'AbortError') reporter?.finishStatus?.('Reading video…', 'failed');
    throw error;
  }
}

export async function prepareBackend(options, { signal, backendResolver = resolveBackend, reporter } = {}) {
  return backendResolver({ signal, onStatus: message => reporter?.status(message), onStatusDone: (message, outcome) => reporter?.finishStatus?.(message, outcome) });
}

/** The extension the finished file will most likely have, for previews. */
export function predictedExtension(options) {
  if (options.audio) return options.format || 'mp3';
  return options.format || 'mp4';
}

/** First free destination name, without creating anything (preview only). */
export async function previewPath(directory, title, extension, { exists = async () => false } = {}) {
  const name = sanitizeTitle(title);
  for (let number = 0; ; number++) {
    const candidate = path.join(directory, `${name}${number ? ` (${number})` : ''}.${extension}`);
    if (!await exists(candidate)) return candidate;
  }
}

/** Summarize source resolutions without downloading media. */
export async function listQualities(options, { signal, backendResolver = resolveBackend, runner = runBackend, reporter } = {}) {
  const backend = await prepareBackend(options, { signal, backendResolver, reporter });
  const metadata = await fetchMetadata({ ...options, playlist: false }, { signal, backend, runner, reporter });
  const formats = (metadata.formats || [metadata]).filter(format => format && format.vcodec !== 'none' && !format.has_drm && format.ext !== 'mhtml' && format.protocol !== 'mhtml');
  if (!formats.length) throw new Error('No downloadable video formats found.');
  const qualities = availableHeights(formats).sort((a, b) => b - a).map(height => height + 'p');
  return { url: options.url, status: 'qualities', title: metadata.title || null, qualities };
}

/** `veo --list-formats`: print the backend's own format table and stop. */
export async function listFormats(options, { signal, backendResolver = resolveBackend, runner = runBackend, reporter } = {}) {
  const backend = await prepareBackend(options, { signal, backendResolver, reporter });
  reporter?.status('Reading available formats…');
  return runner(backend.ytDlp, [...backendArgs(options, backend), '-F', '--', options.mediaUrl || options.url], { signal });
}

/**
 * `veo --dry-run`: inspect the video and report what would happen without
 * downloading or writing anything.
 */
export async function planDownload(options, { signal, backendResolver = resolveBackend, runner = runBackend, reporter } = {}) {
  const directory = path.resolve(options.output);
  const backend = await prepareBackend(options, { signal, backendResolver, reporter });
  const metadata = await fetchMetadata(options, { signal, backend, runner, reporter });
  const quality = selectQuality({ quality: options.quality, audio: options.audio, closest: options.closestQuality, formats: metadata.formats, playlist: options.playlist });
  if (quality.error) throw new Error(quality.error);
  const selected = metadata.entries ? selectedEntries(metadata, options.playlistItems) : [{ entry: metadata, index: 1 }];
  const titles = options.playlist && metadata.entries?.length
    ? selected.map(({ entry, index }) => options.rename?.includes('*') ? stagedTitle('media.mp4', entry, options.rename) : options.rename ? `${options.rename} - ${String(index).padStart(3, '0')}` : entry?.title || `${metadata.title || 'video'} - ${index}`)
    : [stagedTitle('media.mp4', metadata, options.rename)];
  const extension = predictedExtension(options);
  const reserved = new Set();
  const exists = async candidate => reserved.has(candidate) || Boolean(await stat(candidate).catch(() => undefined));
  const planned = [];
  for (const [offset, title] of titles.entries()) {
    const { entry, index } = selected[offset];
    const destination = mediaDestination({ ...options, _entryIndex: metadata.entries ? index : options._entryIndex, _playlistTitle: metadata.entries ? metadata.title : options._playlistTitle }, entry, title);
    await prepareDestination(directory, destination.directory);
    const target = await previewPath(destination.directory, destination.title, extension, { exists });
    reserved.add(target);
    planned.push({ title, path: target });
  }
  return { url: options.url, playlist: Boolean(options.playlist), quality: quality.label, entries: planned, ...sizeEstimate(selected) };
}

async function downloadCollection(options, metadata, dependencies) {
  const { reporter, signal } = dependencies;
  const entries = selectedEntries(metadata, options.playlistItems);
  reporter?.status(describeEstimate(sizeEstimate(entries)));
  const results = new Array(entries.length), failures = [], entryTimings = [];
  let saved = 0, skipped = 0;
  let next = 0;
  let notifications = Promise.resolve();
  const concurrency = options.playlistConcurrency ?? 2;
  if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > 4) throw new Error('Playlist concurrency must be between 1 and 4.');
  const notify = entry => {
    const pending = notifications.then(() => dependencies.onEntry?.(entry));
    notifications = pending.catch(() => {});
    return pending;
  };
  const worker = async workerIndex => {
    while (next < entries.length && workerIndex < reducedLimit(concurrency, dependencies.adaptiveState)) {
      signal?.throwIfAborted();
      const offset = next++;
      const { entry, index } = entries[offset];
      const title = entry?.title || `Entry ${index}`;
      const childReporter = concurrency > 1 ? reporter?.scoped?.(index, metadata.entries.length, title) || reporter : reporter;
      if (concurrency === 1) reporter?.item?.(offset + 1, entries.length, title);
      const itemOptions = { ...options, _entryIndex: index, _playlistTitle: metadata.title || metadata.id,
        rename: options.rename?.includes('*') ? options.rename : options.rename ? `${options.rename} - ${String(index).padStart(3, '0')}` : undefined };
      let outcome;
      try {
        const result = await download(itemOptions, { ...dependencies, reporter: childReporter, skipCacheCleanup: true });
        results[offset] = result.files;
        if (result.timings) entryTimings.push({ index, timings: result.timings });
        if (result.status === 'skipped') skipped++; else saved++;
        outcome = { index, status: result.status || 'saved', files: result.files, ...(result.timings ? { timings: result.timings } : {}) };
      } catch (error) {
        if (signal?.aborted) throw error;
        failures.push({ index, error: error.message });
        reporter?.status(`Entry ${index} failed: ${error.message}`);
        outcome = { index, status: 'failed', error: error.message };
      }
      await notify(outcome);
    }
  };
  // Drain every active worker before returning, including after cancellation.
  const settled = await Promise.allSettled(Array.from({ length: Math.min(concurrency, entries.length) }, (_, index) => worker(index)));
  const rejected = settled.find(result => result.status === 'rejected');
  if (rejected) throw rejected.reason;
  const files = results.flatMap(files => files || []);
  failures.sort((a, b) => a.index - b.index);
  return { url: options.url, title: metadata.title || metadata.id || 'Playlist', files, saved, skipped, failures, ...(entryTimings.length ? { entryTimings: entryTimings.sort((a, b) => a.index - b.index) } : {}),
    status: failures.length ? 'failed' : saved ? 'saved' : 'skipped' };
}

/**
 * Download one URL. Returns every file that was saved, in the order the
 * backend produced them, so collections and subtitle sidecars are reported.
 */
export async function download(options, { signal, reporter, backendResolver = resolveBackend, runner = runBackend, onEntry, localRoot = downloadCacheRoot(), skipCacheCleanup = false, adaptiveState = { divisor: 1 }, wait, diskChecker = reserveSpace, now, compatibilityRunner = runBackend } = {}) {
  const timer = phaseTimer(now);
  const timingResult = () => {
    if (options.timings === false) return {};
    return { timings: timer.result() };
  };
  let directory = path.resolve(options.output);
  localRoot = path.resolve(localRoot);
  if (!skipCacheCleanup) await cleanupDownloadCache(localRoot);
  const requestKey = localRequestKey(options);
  const stagingPath = path.join(localRoot, `${PARTIAL_PREFIX}${requestKey}`);
  const existingStage = await lstat(stagingPath).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  if (existingStage && (!existingStage.isDirectory() || existingStage.isSymbolicLink())) throw new Error(`The partial download path is not a plain directory: ${stagingPath}`);
  const cached = await readJson(path.join(stagingPath, 'job.json'), null);
  const readyToTransfer = cached?.requestKey === requestKey && cached.ready?.length && cached.metadata
    && (cached.backendSucceeded === true || cached.compatibilityChecked === true || cached.files?.length);
  const backend = readyToTransfer ? null : await prepareBackend(options, { signal, backendResolver, reporter });
  timer.switch('metadata');
  const metadata = readyToTransfer ? cached.metadata : await adaptiveRun(() => fetchMetadata(options, { signal, backend, runner, reporter }), { enabled: options.adaptiveConcurrency !== false, state: adaptiveState, signal, reporter, wait, timer });
  if (metadata.entries && !options._entryIndex) return downloadCollection(options, metadata, { signal, reporter, backendResolver: async () => backend, runner, onEntry, localRoot, adaptiveState, wait, diskChecker, now, compatibilityRunner });
  const destination = mediaDestination(options, metadata, stagedTitle('media.mp4', metadata, options.rename));
  directory = destination.directory;
  await prepareDestination(options.output, directory);
  const playlist = false;
  reporter?.name?.(destination.title);
  const quality = selectQuality({ quality: options.quality, audio: options.audio, closest: options.closestQuality, formats: metadata.formats, playlist });
  if (quality.error) throw new Error(quality.error);
  if (quality.label) reporter?.status(quality.label);

  const key = partialKey(metadata, options.url, options);
  const historyFile = path.join(directory, '.veo-history', `${key}.json`);
  const history = options.skipExisting || (options.resume && options._entryIndex) ? await readJson(historyFile, null) : null;
  if (await pruneHistoryRecord(historyFile, directory, history)) {
    reporter?.status('Already downloaded; skipped.');
    return { url: options.url, title: metadata.title, files: [], status: 'skipped', saved: 0, skipped: 1, ...timingResult() };
  }
  const staging = await prepareStaging(stagingPath);
  const lockPath = path.join(staging, '.lock');
  let lock;
  try { lock = await open(lockPath, 'wx'); }
  catch (error) {
    if (error.code === 'EEXIST') throw new Error(`This download is already active, or a force-killed process left ${lockPath}. Remove that lock only after checking that no other veo process uses it.`);
    throw error;
  }
  const manifestFile = path.join(staging, 'job.json');
  let keepPartial = false;
  let unconfirmedOutput = Boolean(!readyToTransfer && (cached?.ready?.length || cached?.unconfirmedOutput));
  let manifest;
  let releaseSpace = () => {};
  try {
    manifest = await readJson(manifestFile, { version: 1, requestKey, metadata: { id: metadata.id, title: metadata.title, channel: metadata.channel, uploader: metadata.uploader, upload_date: metadata.upload_date, playlist_title: options._playlistTitle || metadata.playlist_title, extractor: metadata.extractor, extractor_key: metadata.extractor_key, formats: metadata.formats?.map(({ height, vcodec, has_drm }) => ({ height, vcodec, has_drm })) }, key, source: sourceKey(metadata, options.url), settings: downloadSettings(options), ready: [], files: [] });
    if (manifest.key !== key || manifest.version !== 1) throw new Error('Partial download belongs to different settings.');
    if (!Array.isArray(manifest.ready) || !Array.isArray(manifest.files)
      || manifest.ready.some(file => typeof file !== 'string' || path.dirname(file) !== staging || !isStagedMedia(path.basename(file)))
      || manifest.files.some(item => typeof item?.source !== 'string' || path.dirname(item.source) !== staging || typeof item.destination !== 'string' || path.dirname(item.destination) !== directory)) {
      throw new Error('Invalid partial download manifest.');
    }
    if (options.checkSpace !== false) {
      const bytes = readyToTransfer ? (await Promise.all(manifest.ready.map(file => stat(file)))).reduce((sum, info) => sum + info.size, 0) : estimateMediaBytes(metadata, options);
      releaseSpace = await diskChecker({ cache: localRoot, destination: directory, bytes, cached: Boolean(readyToTransfer), reporter });
    }
    delete manifest.expiresAt;
    await writeJson(manifestFile, manifest);
    // after_move can still be emitted by yt-dlp after a media stream failed.
    // A successful process exit must confirm candidates before transfer/reuse.
    let staged = readyToTransfer ? [...new Set(manifest.ready)] : [];
    if (!staged.length) {
      const unconfirmed = manifest.backendSucceeded === false || manifest.ready.length > 0;
      manifest.ready = [];
      manifest.backendSucceeded = false;
      await writeJson(manifestFile, manifest);
      reporter?.status(options.audio ? 'Downloading audio…' : 'Downloading…');
      timer.switch('download');
        await adaptiveRun(async attempt => {
          const candidates = [];
          const attemptOptions = { ...options, resume: options.resume || attempt > 0, concurrentFragments: reducedLimit(options.concurrentFragments ?? 8, adaptiveState) };
          const args = [...backendArgs(attemptOptions, backend), ...mediaArgs(attemptOptions, quality), '-o', stagingTemplate(staging, false), '--', options.mediaUrl || options.url];
          // A failed process may have left a final filename: --no-overwrites
          // would otherwise silently accept that unconfirmed file on retry.
          if (unconfirmed || attempt > 0) args.splice(args.indexOf('--'), 0, '--force-overwrites');
          await runner(backend.ytDlp, args, { signal, onLine(line) {
            if (line.startsWith('veo-progress:')) {
              if (timer.phase !== 'download') timer.switch('download');
              const data = JSON.parse(line.slice('veo-progress:'.length));
              reporter?.progress(data.progress ? { ...data.progress, stream: data.video === 'none' ? 'Audio' : data.audio === 'none' ? 'Video' : 'Media' } : data);
            }
            if (line.startsWith('veo-postprocess:')) {
              if (timer.phase !== 'processing') timer.switch('processing');
              reporter?.processing?.(JSON.parse(line.slice('veo-postprocess:'.length)));
            }
            if (line.startsWith('veo-file:')) {
              const file = JSON.parse(line.slice('veo-file:'.length));
              if (path.dirname(path.resolve(file)) !== staging || !isStagedMedia(path.basename(file))) throw new Error('The backend returned an invalid saved file path.');
              candidates.push(file);
              unconfirmedOutput = true;
            }
          } });
          staged = [...new Set(candidates)];
        }, { enabled: options.adaptiveConcurrency !== false, state: adaptiveState, signal, reporter, wait, timer });
      manifest.ready = staged;
      manifest.backendSucceeded = true;
      delete manifest.unconfirmedOutput;
      unconfirmedOutput = false;
      await writeJson(manifestFile, manifest);
    }
    // Cancellation must win over any follow-up error so the caller can report
    // "Cancelled." instead of a confusing backend message.
    signal?.throwIfAborted();
    if (!staged.length) throw new Error('The backend finished without producing a file.');

    if (!options.audio && !manifest.compatibilityChecked && (!readyToTransfer || options.compatible)) {
      timer.switch('processing');
      const tools = backend || await prepareBackend(options, { signal, backendResolver, reporter });
      for (let index = 0; index < staged.length; index++) staged[index] = await ensureCompatibility(staged[index], { backend: tools, runner: compatibilityRunner, signal, convert: Boolean(options.compatible), reporter });
      manifest.ready = staged;
      manifest.compatibilityChecked = true;
      await writeJson(manifestFile, manifest);
    }
    const files = [];
    timer.switch('saving');
    reporter?.status(readyToTransfer ? 'Retrying transfer from local cache…' : 'Saving local download to destination…');
    await prepareDestination(options.output, directory, { create: true });
    for (const stagedFile of staged) {
      const resolved = path.resolve(stagedFile);
      // The backend must only ever hand back a file inside our staging directory.
      if (path.dirname(resolved) !== staging || !(await lstat(resolved)).isFile()) throw new Error('The backend returned an invalid saved file path.');
      let record = manifest.files.find(item => item.source === resolved);
      if (record && !await stat(record.destination).then(info => info.isFile(), () => false)) record = null;
      const saved = record?.destination || await saveUnique(resolved, directory, destination.title, { signal, keepSource: true });
      if (!record) {
        manifest.files = manifest.files.filter(item => item.source !== resolved);
        manifest.files.push({ source: resolved, destination: saved });
        await writeJson(manifestFile, manifest);
      }
      files.push(saved);
      files.push(...await saveSidecars(staging, resolved, saved, { signal, manifest, manifestFile }));
    }
    await writeJson(historyFile, { version: 1, source: sourceKey(metadata, options.url), settings: downloadSettings(options), files });
    return { url: options.url, title: metadata.title || metadata.id || 'video', files, status: 'saved', saved: 1, skipped: 0, ...timingResult() };
  } catch (error) {
    if (!signal?.aborted) reporter?.failStep?.();
    // A kept staging directory is the whole point of --resume, and cancellation
    // is the most common reason to want one.
    keepPartial = Boolean(options.resume || manifest?.ready?.length || unconfirmedOutput);
    if (manifest && unconfirmedOutput) manifest.unconfirmedOutput = true;
    if (manifest?.ready?.length || (unconfirmedOutput && !options.resume)) {
      manifest.expiresAt = Date.now() + TRANSFER_RETENTION_MS;
      await writeJson(manifestFile, manifest);
    }
    throw error;
  } finally {
    releaseSpace();
    await lock.close();
    await rm(lockPath, { force: true });
    reporter?.finish();
    if (keepPartial) reporter?.status(manifest?.expiresAt && !manifest?.ready?.length
      ? `Unconfirmed download kept locally in ${staging} for 15 minutes. Retry will rerun the backend; incomplete media may need to be downloaded again.`
      : manifest?.expiresAt
      ? `Completed download kept locally in ${staging}. Retry within 15 minutes to transfer without downloading again. Expired files are cleaned on the next veo run.`
      : `Partial download kept locally in ${staging}. Re-run with --resume to continue it.`);
    else await rm(staging, { recursive: true, force: true });
  }
}

/**
 * Move subtitle and thumbnail files beside the saved media file, keeping the
 * media name as the prefix (Title.mp4 -> Title.en.vtt). Collections pair each
 * file with the sidecars that share its index.
 */
async function saveSidecars(staging, stagedMedia, savedMedia, { signal, manifest, manifestFile } = {}) {
  const written = [];
  const stem = path.basename(stagedMedia, path.extname(stagedMedia));
  const base = path.basename(savedMedia, path.extname(savedMedia));
  const entries = await readdir(staging, { withFileTypes: true }).catch(() => []);
  for (const entry of entries) {
    if (!entry.isFile() || !isSidecar(entry.name) || !entry.name.startsWith(`${stem}.`)) continue;
    const suffix = path.basename(entry.name, path.extname(entry.name)).slice(stem.length);
    const source = path.join(staging, entry.name);
    const record = manifest.files.find(item => item.source === source);
    if (record && await stat(record.destination).then(info => info.isFile(), () => false)) {
      written.push(record.destination);
      continue;
    }
    const destination = await saveUnique(source, path.dirname(savedMedia), `${base}${suffix}`, { signal, keepSource: true });
    manifest.files = manifest.files.filter(item => item.source !== source);
    manifest.files.push({ source, destination });
    await writeJson(manifestFile, manifest);
    written.push(destination);
  }
  return written;
}
