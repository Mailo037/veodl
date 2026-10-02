// Real, authorized end-to-end test: generate our own media, serve it locally,
// and download through the actual CLI and yt-dlp. No third-party content.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { chmod, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { exeSuffix, resolveMediaTools } from '../src/backend.js';

const cli = fileURLToPath(new URL('../bin/veo.js', import.meta.url));
const root = await mkdtemp(path.join(os.tmpdir(), 'veo-smoke-'));
// Keep configuration and job records inside this disposable fixture.
process.env.VEO_CONFIG = path.join(root, 'config.json');
process.env.VEO_NO_UPDATE_CHECK = '1';
process.env.LOCALAPPDATA = root;
process.env.XDG_CACHE_HOME = root;
function run(command, args, cwd = root) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: 'inherit', shell: false });
    child.on('error', reject);
    child.on('close', code => resolve(code));
  });
}
// Captures output for the machine-readable and inspection modes.
function capture(command, args, cwd = root) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'], shell: false });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', data => { stdout += data; });
    child.stderr.on('data', data => { stderr += data; });
    child.on('error', reject);
    child.on('close', code => resolve({ code, stdout, stderr }));
  });
}
let server;
let recovered = false;
let requests = 0;
try {
  const toolCache = path.join(root, 'smoke-tools');
  await mkdir(toolCache);
  const mediaDirectory = await resolveMediaTools({ directory: toolCache });
  if (process.platform !== 'win32' && mediaDirectory === toolCache) {
    // A valid managed cache with lost executable bits must be repairable offline.
    for (const name of ['ffmpeg', 'ffprobe']) await chmod(path.join(toolCache, name), 0o600);
    assert.equal(await resolveMediaTools({ directory: toolCache, offline: true,
      find: async () => { throw new Error('Verified cache must not need PATH'); } }), toolCache);
  }
  const ffmpeg = path.join(mediaDirectory, `ffmpeg${exeSuffix()}`);
  const source = path.join(root, 'fixture.mp4');
  assert.equal(await run(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=640x360:r=24', '-f', 'lavfi', '-i', 'sine=frequency=440', '-t', '2', '-c:v', 'libx264', '-c:a', 'aac', '-movflags', '+faststart', source]), 0);
  const media = await readFile(source);
  server = createServer((req, res) => {
    requests++;
    if (req.url === '/playlist.html') {
      res.writeHead(200, { 'Content-Type': 'text/html' });
      res.end('<html><head><title>Fixture playlist</title></head><body><video src="/original-title.mp4"></video><video src="/recover.mp4"></video></body></html>');
      return;
    }
    if (req.url !== '/original-title.mp4' && !(req.url === '/recover.mp4' && recovered)) { res.writeHead(404); res.end(); return; }
    res.writeHead(200, { 'Content-Type': 'video/mp4', 'Content-Length': media.length });
    res.end(req.method === 'HEAD' ? undefined : media);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/original-title.mp4`;
  assert.equal(await run(process.execPath, [cli, '--help']), 0);
  assert.equal(await run(process.execPath, [cli, '--version']), 0);
  assert.equal(await run(process.execPath, [cli, 'not-a-url']), 1);
  assert.equal(await run(process.execPath, [cli, url]), 0);
  assert.equal(await run(process.execPath, [cli, url, '-q', '720p']), 0);
  assert.equal(await run(process.execPath, [cli, url, '--audio', '-o', './audio']), 0);
  assert.equal(await run(process.execPath, [cli, url, '-r', 'My Video']), 0);
  assert.equal(await run(process.execPath, [cli, url, '-r', 'My Video']), 0);
  assert.equal(await run(process.execPath, [cli, url, '--audio', '--rename', 'My Music', '-o', './audio']), 0);
  assert.equal(await run(process.execPath, [cli, url, '--format', 'webm', '--recode', '-o', './converted']), 0);
  assert.equal(await run(process.execPath, [cli, url, '--format', 'mkv', '-o', './remuxed']), 0);
  const streamHashes = file => capture(ffmpeg, ['-v', 'error', '-i', file, '-map', '0', '-c', 'copy', '-f', 'streamhash', '-hash', 'sha256', '-']);
  const originalHashes = await streamHashes(source);
  const remuxedHashes = await streamHashes(path.join(root, 'remuxed', 'original-title.mkv'));
  assert.equal(originalHashes.code, 0);
  assert.equal(remuxedHashes.code, 0);
  assert.equal(remuxedHashes.stdout, originalHashes.stdout, 'remux must preserve encoded video and audio packets');
  const incompatible = await capture(process.execPath, [cli, url, '--format', 'webm', '-o', './incompatible']);
  assert.equal(incompatible.code, 1, 'incompatible container must fail instead of silently re-encoding');
  assert.equal(await run(process.execPath, [cli, url.replace('original-title.mp4', 'deleted.mp4')]), 1);

  // Inspection modes must answer without writing anything.
  const dry = await capture(process.execPath, [cli, url, '--dry-run', '-o', './dry']);
  assert.equal(dry.code, 0);
  assert.match(dry.stdout, /Would save: /);
  assert(!(await readdir(root)).includes('dry'), 'dry run must not create the output directory');
  const formats = await capture(process.execPath, [cli, url, '--list-formats']);
  assert.equal(formats.code, 0);
  assert.match(formats.stdout, /mp4/);
  const json = await capture(process.execPath, [cli, url, '--json', '-o', './json']);
  assert.equal(json.code, 0);
  const parsed = JSON.parse(json.stdout.trim().split('\n').pop());
  assert.equal(parsed.status, 'saved');
  assert.equal(parsed.files.length, 1);
  assert.equal((await stat(parsed.files[0])).size, media.length);

  // Metadata embedding, resume on a finished download, and batch behaviour.
  assert.equal(await run(process.execPath, [cli, url, '--embed-metadata', '-o', './tagged']), 0);
  assert((await stat(path.join(root, 'tagged', 'original-title.mp4'))).size > 0);
  assert.equal(await run(process.execPath, [cli, url, '--resume', '-o', './resumed']), 0);
  const batch = await capture(process.execPath, [cli, url, url.replace('original-title.mp4', 'deleted.mp4'), '-o', './batch']);
  assert.equal(batch.code, 1, 'a batch with one failing URL must report failure');
  assert.match(batch.stderr, /deleted\.mp4/, 'the failing URL must be named');
  assert.equal((await stat(path.join(root, 'batch', 'original-title.mp4'))).size, media.length);
  assert(!batch.stdout.includes('deleted'), 'only successful saves are printed on stdout');

  // Profiles + list files + a persisted retry from a different working directory.
  await writeFile(path.join(root, 'config.json'), JSON.stringify({ profiles: { small: { quality: '720p', output: path.join(root, 'profile-output') } } }));
  const list = path.join(root, 'urls.txt');
  await writeFile(list, `# Own generated videos\n${url}\n\n${url.replace('original-title.mp4', 'recover.mp4')}\n`);
  const fromList = await capture(process.execPath, [cli, '--batch-file', list, '--profile', 'small', '--json']);
  assert.equal(fromList.code, 1, fromList.stderr);
  const listResults = fromList.stdout.trim().split('\n').map(line => JSON.parse(line));
  assert.deepEqual(listResults.map(result => result.status).sort(), ['failed', 'saved']);
  const retryId = listResults.find(result => result.status === 'failed').runId;
  assert.match(retryId, /^[a-z0-9]{6}$/);
  recovered = true;
  const otherCwd = path.join(root, 'other-cwd');
  await mkdir(otherCwd);
  const retried = await capture(process.execPath, [cli, '--retry-failed', retryId, '--json'], otherCwd);
  assert.equal(retried.code, 0, retried.stderr);
  const retryResult = JSON.parse(retried.stdout.trim());
  assert.equal(retryResult.status, 'saved');
  assert.equal(path.dirname(retryResult.files[0]), path.join(root, 'profile-output'));
  const skipped = await capture(process.execPath, [cli, url, '--profile', 'small', '--skip-existing', '--json']);
  assert.equal(skipped.code, 0, skipped.stderr);
  assert.equal(JSON.parse(skipped.stdout.trim()).status, 'skipped');

  // `veo history` reports the attempts of the real runs above, newest first.
  const history = await capture(process.execPath, [cli, 'history', '--json', '--limit', '100']);
  assert.equal(history.code, 0, history.stderr);
  const recorded = JSON.parse(history.stdout);
  assert(recorded.count > 0, 'a run must leave history');
  assert.equal(recorded.entries[0].status, 'skipped', 'the newest attempt is listed first');
  assert(recorded.entries.some(entry => entry.status === 'failed' && /deleted\.mp4/.test(entry.url)), 'a failed attempt must be recorded');
  assert(recorded.entries.some(entry => entry.status === 'saved' && entry.files.length === 1), 'a saved attempt must record its files');
  const historyText = await capture(process.execPath, [cli, 'history']);
  assert.equal(historyText.code, 0, historyText.stderr);
  assert.match(historyText.stdout, /^veo history \(last 5, newest first\)/);
  assert.match(historyText.stdout, /Status: skipped/);

  // Finished runs must clean up their record, so nothing is left to stop.
  const leftover = await capture(process.execPath, [cli, 'runs']);
  assert.equal(leftover.code, 0, leftover.stderr);
  assert.match(leftover.stdout, /No veo runs are active/);
  const nothingToStop = await capture(process.execPath, [cli, 'stop']);
  assert.equal(nothingToStop.code, 0, nothingToStop.stderr);
  assert.match(nothingToStop.stdout, /No veo runs were active/);

  // Generic HTML playlist exercises actual --playlist-items extraction and per-entry saves.
  const playlist = await capture(process.execPath, [cli, url.replace('original-title.mp4', 'playlist.html'), '--playlist-items', '2', '-r', 'Selected', '-o', './playlist', '--resume', '--json']);
  assert.equal(playlist.code, 0, playlist.stderr);
  const playlistResult = JSON.parse(playlist.stdout.trim());
  assert.equal(playlistResult.files.length, 1);
  assert.equal(path.basename(playlistResult.files[0]), 'Selected - 002.mp4');

  // A finished download survives an unavailable destination, even without --resume.
  const blockedDestination = path.join(root, 'blocked-output');
  await writeFile(blockedDestination, 'destination unavailable');
  const failedTransfer = await capture(process.execPath, [cli, url, '-o', blockedDestination, '--json']);
  assert.equal(failedTransfer.code, 1, failedTransfer.stderr);
  assert.match(failedTransfer.stderr, /Completed download kept locally/);
  await rm(blockedDestination);
  const beforeRetry = requests;
  const transferRetry = await capture(process.execPath, [cli, url, '-o', blockedDestination, '--json']);
  assert.equal(transferRetry.code, 0, transferRetry.stderr);
  assert.equal(requests, beforeRetry, 'transfer retry must not contact the source again');
  assert.equal((await stat(JSON.parse(transferRetry.stdout.trim()).files[0])).size, media.length);
  const files = await readdir(root);
  assert(files.includes('original-title.mp4'));
  assert(files.includes('original-title (1).mp4'));
  assert.equal((await stat(path.join(root, 'My Video.mp4'))).size, media.length);
  assert.equal((await stat(path.join(root, 'My Video (1).mp4'))).size, media.length);
  assert((await stat(path.join(root, 'audio', 'My Music.mp3'))).size > 0);
  assert.equal((await stat(path.join(root, 'original-title.mp4'))).size, media.length);
  assert((await stat(path.join(root, 'audio', 'original-title.mp3'))).size > 0);
  assert((await stat(path.join(root, 'converted', 'original-title.webm'))).size > 0);
  assert(!files.some(name => name.startsWith('.veo-') && name !== '.veo-history'));
  console.log('PASS: real video, quality cap, duplicate names, audio, conversion, dry run, formats, JSON, metadata, resume, batch failure, profiles, URL lists, retry from another directory, skip-existing, playlist selection, history and run listings.');
} finally {
  if (server) await new Promise(resolve => server.close(resolve));
  await rm(root, { recursive: true, force: true });
}
