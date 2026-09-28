import { runPool, serialQueue, reducedLimit, formatTimings } from './execution.js';
import { styleText, terminalText } from './progress.js';
import path from 'node:path';
import { lstat, mkdtemp, readdir, rm } from 'node:fs/promises';
import os from 'node:os';
import { randomUUID } from 'node:crypto';
import { cacheBase } from './paths.js';
import { publicOptions, readJson, writeJson } from './state.js';
import { cleanText, readableError, validateUrl } from './utils.js';
import { RUN_ARCHIVE_ID } from './run-archive.js';
import { folderLink } from './path-links.js';

export async function retryOptions(file, root = cacheBase()) {
  if (RUN_ARCHIVE_ID.test(file)) file = await retryJobForRun(file, root);
  const job = await readJson(path.resolve(file), null);
  if (job?.version !== 1 || !Array.isArray(job.items)) throw new Error('Invalid retry job file.');
  const pending = job.items.filter(item => !['saved', 'skipped'].includes(item.status));
  if (!pending.length) throw new Error('This job has no failed or unfinished downloads.');
  return pending.map(item => {
    validateUrl(item.url);
    const entries = item.entries || [];
    // After interruption, retry the original selection; resume history skips completed entries.
    const failures = entries.filter(entry => entry.status === 'failed');
    return { ...job.options, ...item.options, url: item.url, resume: true,
      ...(item.status === 'failed' && failures.length && item.finished ? { playlistItems: failures.map(entry => entry.index).join(',') } : {}) };
  });
}

/** Resolve a persistent run ID using its original retry job, including older runs. */
async function retryJobForRun(id, root) {
  const { listRuns } = await import('./runs.js');
  const runs = await listRuns(root);
  if (runs.some(run => run.id === id && run.alive)) throw new Error(`Run ${id} is still active. Stop it before retrying.`);
  const directory = path.join(path.resolve(root), 'jobs');
  const entries = await readdir(directory, { withFileTypes: true }).catch(error => {
    if (error.code === 'ENOENT') return [];
    throw error;
  });
  for (const entry of entries.filter(entry => entry.isFile() && /^\d{13}-[a-f0-9-]{36}\.json$/.test(entry.name)).sort((a, b) => b.name.localeCompare(a.name))) {
    const file = path.join(directory, entry.name);
    const job = await readJson(file, null).catch(error => { if (error instanceof SyntaxError) return null; throw error; });
    if (job?.runId === id) return file;
  }
  throw new Error(`No retry job found for run ${id}. Retry jobs may have been removed by veo flush.`);
}

/**
 * Location of one run's job file. It is created before the first download starts
 * so `veo runs <id>` can report per-item progress from it.
 */
export function jobFilePath(root = cacheBase()) {
  return path.join(root, 'jobs', `${Date.now()}-${randomUUID()}.json`);
}

/** Find the newest retryable job, skipping finished jobs and active runs. */
export async function latestFailedJob(root = cacheBase()) {
  const directory = path.join(path.resolve(root), 'jobs');
  const info = await lstat(directory).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  if (!info) throw new Error('No retry jobs found. Run a download first.');
  if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`Invalid retry job directory: ${directory}`);
  const { listRuns } = await import('./runs.js');
  const active = new Set((await listRuns(root)).filter(run => run.alive && run.job).map(run => path.resolve(run.job)));
  const names = (await readdir(directory, { withFileTypes: true }))
    .filter(entry => entry.isFile() && /^\d{13}-[a-f0-9-]{36}\.json$/.test(entry.name))
    .map(entry => entry.name).sort().reverse();
  for (const name of names) {
    const file = path.join(directory, name);
    if (active.has(file)) continue;
    const job = await readJson(file, null).catch(error => { if (error instanceof SyntaxError) return null; throw error; });
    if (job?.version === 1 && job.options && typeof job.options === 'object' && !Array.isArray(job.options)
      && Array.isArray(job.items) && job.items.every(item => item && typeof item.url === 'string' && typeof item.status === 'string')
      && job.items.some(item => !['saved', 'skipped'].includes(item.status))) return file;
  }
  throw new Error('No failed or unfinished retry job found.');
}

export async function runJob(options, { download, reporter, signal, openFile, stdout = process.stdout, stderr = process.stderr, jobFile, items, recordStats, recordHistory, runId, archiveRoot } = {}) {
  if (options.incognito) {
    runId = undefined;
    recordStats = undefined;
    recordHistory = undefined;
  }
  const concurrency = options.concurrentDownloads ?? 2;
  if (!Number.isInteger(concurrency) || concurrency < 1 || concurrency > 4) throw new Error('Concurrent downloads must be between 1 and 4.');
  const file = jobFile || jobFilePath();
  const requests = items || options.urls.map(url => ({ ...options, url }));
  const job = { version: 1, ...(runId ? { runId } : {}), startedAt: new Date().toISOString(), options: publicOptions({ ...options, output: path.resolve(options.output) }),
    items: requests.map(item => ({ url: item.url, status: 'pending', entries: [] })) };
  // Each retry item can have its own playlist selection.
  job.items.forEach((item, index) => { item.options = publicOptions(requests[index]); });
  const { cleanupDownloadCache } = await import('./download-cache.js');
  if (!options.incognito) await cleanupDownloadCache();
  const privateRoot = options.incognito ? await mkdtemp(path.join(os.tmpdir(), 'veo-incognito-')) : null;
  const queue = serialQueue();
  const persist = () => options.incognito ? Promise.resolve() : queue(() => writeJson(file, job));
  await persist();
  const activeTargets = new Map();
  const totals = { saved: 0, skipped: 0, failed: 0 };
  const adaptiveState = { divisor: 1 };
  const archiveTools = runId ? await import('./run-archive.js') : null;
  try {
    await runPool(requests, () => reducedLimit(concurrency, adaptiveState), async (request, index) => {
    const targetKey = JSON.stringify([request.url, path.resolve(request.output)]);
    const previous = activeTargets.get(targetKey);
    let release;
    const current = new Promise(resolve => { release = resolve; });
    activeTargets.set(targetKey, current);
    try {
      await previous;
      signal?.throwIfAborted();
      let saved = 0, skipped = 0, failed = 0;
      const itemReporter = concurrency > 1 && requests.length > 1 ? reporter.scoped?.(index + 1, requests.length, request.url) || reporter : reporter;
      const item = job.items[index];
      const captureFiles = async files => {
        if (!archiveTools) return;
        item.fingerprints ||= {};
        for (const target of files || []) {
          if (!archiveTools.isMediaFile(target) || item.fingerprints[target]) continue;
          item.fingerprints[target] = await archiveTools.fileFingerprint(target).catch(() => null);
        }
      };
      const started = performance.now();
      const published = new Set();
      let opened = false;
      let title;
      const publish = async files => {
        if (!options.json) for (const target of files) {
          if (!published.has(target)) {
            const display = terminalText(stdout, `Saved: ${cleanText(target)}`);
            stdout.write(styleText(stdout, display.startsWith('Saved: ') ? `Saved: ${folderLink(stdout, target, display.slice(7))}` : display, 'success', options.color !== false) + '\n');
          }
          published.add(target);
        }
        if (request.open && !opened && files.length) {
          opened = true;
          try { await openFile(files[0]); }
          catch (error) { stderr.write(`veo: File saved, but could not launch the default app: ${readableError(error)}\n`); }
        }
      };
      if (itemReporter === reporter) reporter.item?.(index + 1, requests.length, request.url);
      try {
        item.status = 'running';
        await persist();
        const result = await download(request, { reporter: itemReporter, signal, adaptiveState, skipCacheCleanup: true,
          ...(privateRoot ? { localRoot: privateRoot } : {}), onEntry: async entry => {
          await captureFiles(entry.files);
          item.entries.push(entry);
          await persist();
          if (entry.status === 'saved') saved++;
          if (entry.status === 'skipped') skipped++;
          if (entry.status === 'failed') failed++;
          await publish(entry.files || []);
        } });
        await captureFiles(result.files);
        Object.assign(item, { status: result.status || 'saved', files: result.files, timings: result.timings, entryTimings: result.entryTimings, finished: true });
        title = result.title;
        if (!item.entries.length) {
          saved += result.saved ?? (item.status === 'saved' ? 1 : 0);
          skipped += result.skipped ?? 0;
          failed += result.failures?.length ?? (item.status === 'failed' ? 1 : 0);
        }
        if (options.json) stdout.write(`${JSON.stringify({ ...result, status: item.status, profile: request.profile || null, ...(runId ? { runId } : {}) })}\n`);
        await publish(result.files);
      } catch (error) {
        item.status = signal?.aborted ? 'cancelled' : 'failed';
        item.error = readableError(error);
        if (!signal?.aborted) failed++;
        item.files = [...new Set([...item.entries.flatMap(entry => entry.files || []), ...(error.files || [])])];
        if (options.json) stdout.write(`${JSON.stringify({ url: request.url, status: item.status, profile: request.profile || null, error: item.error, files: item.files, ...(runId ? { runId } : {}) })}\n`);
        else stderr.write(styleText(stderr, terminalText(stderr, `veo: ${item.error} (${request.url})`), 'error', options.color !== false) + '\n');
        if (error.verificationFailed && !options.json) for (const file of item.files) stderr.write(`Saved file retained: ${cleanText(file)}\n`);
      }
      if (recordStats) {
        try {
          await recordStats({ videos: request.audio ? 0 : saved,
            audio: request.audio ? saved : 0, failed: failed,
            skipped: skipped, cancelled: item.status === 'cancelled' ? 1 : 0,
            elapsedMs: Math.round(performance.now() - started) });
        } catch (error) { stderr.write(`veo: Could not save statistics: ${readableError(error)}\n`); }
      }
      // History is written per finished item, so `veo history` never reports an
      // attempt that is still running.
      if (recordHistory) {
        try {
          await recordHistory({ url: request.url, title, status: item.status, runId, job: ['failed', 'cancelled'].includes(item.status) ? file : null, audio: Boolean(request.audio),
            quality: request.quality, format: request.format, files: item.files || [], error: item.error,
            elapsedMs: Math.round(performance.now() - started) });
        } catch (error) { stderr.write(`veo: Could not save download history: ${readableError(error)}\n`); }
      }
      await persist();
      totals.saved += saved; totals.skipped += skipped; totals.failed += failed;
    } finally { release(); if (activeTargets.get(targetKey) === current) activeTargets.delete(targetKey); }
    }, { signal }).catch(error => { if (!signal?.aborted) throw error; });
  } finally {
    if (privateRoot) await rm(privateRoot, { recursive: true, force: true });
  }
  const { saved, skipped, failed } = totals;
  if (runId) {
    try {
      await archiveTools.archiveRun(job, { ...(archiveRoot ? { root: archiveRoot } : {}) });
    } catch (error) { stderr.write(`veo: Could not archive run ${runId}: ${readableError(error)}\n`); }
    stderr.write(`Run ID: ${runId}\n`);
  }
  const unfinished = job.items.some(item => !['saved', 'skipped'].includes(item.status));
  const downloadCount = job.items.reduce((count, item) => count + Math.max(1, item.entries?.length || 0), 0);
  if (downloadCount > 1) stderr.write(styleText(stderr, `Summary: ${saved} saved, ${skipped} skipped, ${failed} failed${signal?.aborted ? ', cancelled' : ''}.`, unfinished ? 'error' : 'success', options.color !== false && !options.json) + '\n');
  if (options.timings !== false) {
    const totals = {};
    for (const item of job.items) for (const timings of [item.timings, ...(item.entryTimings || []).map(entry => entry.timings)]) {
      if (!timings) continue;
      for (const [phase, ms] of Object.entries(timings)) totals[phase] = (totals[phase] || 0) + ms;
    }
    if (Object.keys(totals).length) stderr.write(styleText(stderr, formatTimings(totals), 'muted', options.color !== false && !options.json) + '\n');
  }
  if (unfinished && !options.incognito) stderr.write(`Retry failed/unfinished downloads: veo --retry-failed ${runId || `"${file}"`}\n`);
  if (unfinished) reporter.fail(Boolean(signal?.aborted)); else reporter.complete();
  return signal?.aborted ? 130 : unfinished ? 1 : 0;
}
