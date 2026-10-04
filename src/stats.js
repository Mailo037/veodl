import { commandOutput } from './output.js';
import { randomUUID } from 'node:crypto';
import { readdir, lstat, rm } from 'node:fs/promises';
import path from 'node:path';
import { cacheBase } from './paths.js';
import { readJson, writeJson } from './state.js';

const fields = ['videos', 'audio', 'failed', 'skipped', 'cancelled', 'elapsedMs'];
const pattern = /^[a-f0-9-]{36}\.json$/;
const empty = () => Object.fromEntries(fields.map(key => [key, 0]));

export function createStatsRecorder(root = cacheBase()) {
  const file = path.join(root, 'stats', `${randomUUID()}.json`);
  const totals = { version: 1, since: new Date().toISOString(), ...empty() };
  return async delta => {
    for (const key of fields) totals[key] += delta[key] || 0;
    await writeJson(file, totals);
  };
}

async function statFiles(root) {
  const directory = path.resolve(root, 'stats');
  const info = await lstat(directory).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
  if (!info) return [];
  if (!info.isDirectory() || info.isSymbolicLink()) throw new Error(`Invalid statistics directory: ${directory}`);
  return (await readdir(directory, { withFileTypes: true }))
    .filter(entry => entry.isFile() && pattern.test(entry.name)).map(entry => path.join(directory, entry.name));
}

export async function readStats(root = cacheBase()) {
  const result = { since: null, ...empty() };
  for (const file of await statFiles(root)) {
    const value = await readJson(file, null);
    if (!value) continue;
    if (value.version !== 1 || !Number.isFinite(Date.parse(value.since)) || fields.some(key => !Number.isFinite(value[key]) || value[key] < 0)) {
      throw new Error(`Invalid statistics file: ${file}`);
    }
    if (!result.since || value.since < result.since) result.since = value.since;
    for (const key of fields) result[key] += value[key];
  }
  return result;
}

// Called by flush only after registered runs have stopped and while its gate is held.
export async function resetStats(root = cacheBase()) {
  for (const file of await statFiles(root)) await rm(file, { force: true });
}

export async function statsMain(args = [], { stdout = process.stdout, root = cacheBase() } = {}) {
  [args, stdout] = commandOutput(args, stdout);
  if (args.length === 1 && ['--help', '-h'].includes(args[0])) {
    stdout.write('veo stats [--json]\n\nShow recorded download totals. Time includes preparation, processing and saving,\nincluding failed attempts. Parallel run times are added together.\nStats start with this version; old downloads are not imported.\nUse veo flush --stats to clear temporary data and reset statistics.\n');
    return 0;
  }
  if (args.length && (args.length !== 1 || args[0] !== '--json')) throw new Error('Usage: veo stats [--json]');
  const stats = await readStats(root);
  if (args[0] === '--json') stdout.write(`${JSON.stringify(stats)}\n`);
  else {
    const seconds = Math.floor(stats.elapsedMs / 1000);
    stdout.write(`veo stats\n\nTracking since: ${stats.since || 'no downloads yet'}\nVideos saved:   ${stats.videos}\nAudio saved:    ${stats.audio}\nTotal failures: ${stats.failed}\nSkipped:        ${stats.skipped}\nCancelled:      ${stats.cancelled}\nDownload time:  ${Math.floor(seconds / 3600)}h ${Math.floor(seconds / 60) % 60}m ${seconds % 60}s\n\nTime includes preparation, processing and saving. Active requests appear when finished.\n`);
  }
  return 0;
}
