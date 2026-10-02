import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream, createWriteStream } from 'node:fs';
import { chmod, lstat, mkdir, mkdtemp, open, rm } from 'node:fs/promises';
import path from 'node:path';
import { Readable, Transform, Writable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { createGunzip } from 'node:zlib';
import { MEDIA_ASSETS, MEDIA_ASSET_BASE } from './media-assets.js';
import { cleanText } from './utils.js';

// Only fixed package names and arguments are passed to these installers.
export function runSetup(command, args, {
  signal, env = process.env, cwd, status = () => {}, timeoutMs = 600_000,
} = {}) {
  signal?.throwIfAborted();
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      shell: false, windowsHide: true, detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'pipe'], env, cwd,
    });
    let output = '';
    let stopped;
    const stop = error => {
      if (stopped) return;
      stopped = error;
      if (process.platform === 'win32' && child.pid) {
        const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { shell: false, windowsHide: true, stdio: 'ignore' });
        killer.on('error', () => child.kill());
      } else {
        try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
      }
    };
    const abort = () => stop(signal.reason);
    const timer = setTimeout(() => stop(new Error('Tool installation timed out. Retry with veo doctor fix.')), timeoutMs);
    const receive = chunk => {
      const text = cleanText(String(chunk));
      output = (output + text).slice(-8192);
      // Surface package-manager progress without taking over stdin or stdout.
      for (const line of text.split(/[\r\n]+/)) if (line.trim()) status(line.slice(0, 500));
    };
    child.stdout.on('data', receive);
    child.stderr.on('data', receive);
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
    const cleanup = () => { clearTimeout(timer); signal?.removeEventListener('abort', abort); };
    child.once('error', error => { cleanup(); reject(stopped || error); });
    child.once('close', code => {
      cleanup();
      if (stopped) reject(stopped);
      else if (code !== 0) reject(new Error(`${path.basename(command)} exited with code ${code}: ${output.trim()}`));
      else resolve(output);
    });
  });
}

export async function hasTermuxEjs({ find, run = runSetup, signal }) {
  const python = await find(['python', 'python3']);
  if (!python) return false;
  try {
    await run(python, ['-B', '-c', 'import yt_dlp_ejs'], { signal, timeoutMs: 15_000 });
    return true;
  } catch {
    signal?.throwIfAborted();
    return false;
  }
}

let termuxInstall;
export async function installTermuxTools({ find, signal, status, run = runSetup, env = process.env }) {
  if (termuxInstall) {
    await termuxInstall;
    signal?.throwIfAborted();
    return;
  }
  termuxInstall = (async () => {
    const pkg = await find(['pkg']);
    if (!pkg) throw new Error('Automatic Android setup requires Termux with pkg on PATH. Install python-yt-dlp, yt-dlp-ejs and ffmpeg in Termux.');
    signal?.throwIfAborted();
    status('Installing yt-dlp, JavaScript support and FFmpeg with Termux pkg (first use)…');
    // pkg refreshes mirrors/package lists itself. No full system upgrade or sudo.
    await run(pkg, ['install', env.TERMUX_APP_PACKAGE_MANAGER === 'pacman' ? '--noconfirm' : '-y', 'python-yt-dlp', 'yt-dlp-ejs', 'ffmpeg'], {
      signal, status, env: { ...env, DEBIAN_FRONTEND: 'noninteractive' },
    });
  })();
  try { await termuxInstall; }
  catch (cause) {
    signal?.throwIfAborted();
    throw new Error(`Automatic Termux setup failed: ${cause.message}. Retry with veo doctor fix; if the repository needs repair, run pkg update in Termux.`, { cause });
  } finally { termuxInstall = undefined; }
}

export function mediaPackagePlan(platform = process.platform, arch = process.arch) {
  // Windows 11 ARM runs the upstream x64 media tools via OS emulation.
  const binaryArch = platform === 'win32' && arch === 'arm64' ? 'x64' : arch;
  const assets = MEDIA_ASSETS[`${platform}-${binaryArch}`];
  return assets ? { binaryArch, ...assets } : undefined;
}

const DOWNLOAD_TIMEOUT_MS = 600_000;
const MEDIA_ORIGINS = new Set(['https://github.com', 'https://release-assets.githubusercontent.com']);

function validateAsset(asset) {
  if (!asset || typeof asset.name !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(asset.name) || asset.name.includes('..')) {
    throw new Error('Invalid media asset name.');
  }
  if (!Number.isSafeInteger(asset.size) || asset.size <= 0 || typeof asset.sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(asset.sha256)) {
    throw new Error(`Invalid pinned integrity metadata for ${asset.name}.`);
  }
}

function integrityStream(asset) {
  validateAsset(asset);
  const hash = createHash('sha256');
  let size = 0;
  return new Transform({
    transform(chunk, _encoding, callback) {
      size += chunk.length;
      if (size > asset.size) return callback(new Error(`Unexpected size for ${asset.name}: exceeds pinned byte count.`));
      hash.update(chunk);
      callback(null, chunk);
    },
    flush(callback) {
      if (size !== asset.size) return callback(new Error(`Unexpected size for ${asset.name}: does not match pinned byte count.`));
      if (hash.digest('hex') !== asset.sha256) return callback(new Error(`Integrity verification failed for ${asset.name}: SHA-256 mismatch.`));
      callback();
    },
  });
}

function downloadUrl(value) {
  const url = new URL(value);
  if (!MEDIA_ORIGINS.has(url.origin) || url.username || url.password) throw new Error('Media download redirect must remain on an approved HTTPS origin.');
  return url;
}

async function mediaResponse(url, { signal, fetchImpl }) {
  let current = downloadUrl(url);
  for (let redirects = 0; redirects <= 5; redirects++) {
    signal.throwIfAborted();
    const response = await fetchImpl(current.href, {
      signal, redirect: 'manual', headers: { 'Accept-Encoding': 'identity' },
    });
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      await response.body?.cancel?.().catch(() => {});
      const location = response.headers.get('location');
      if (!location || redirects === 5) throw new Error('Media download has an invalid or excessive redirect.');
      current = downloadUrl(new URL(location, current));
      continue;
    }
    if (response.status !== 200) {
      await response.body?.cancel?.().catch(() => {});
      throw new Error(`Media download failed with HTTP ${response.status}.`);
    }
    if (!response.body) throw new Error('Media download response has no body.');
    return response;
  }
  throw new Error('Media download has an excessive redirect.');
}

// This reusable primitive receives pinned metadata from its caller. Desktop
// setup supplies only the release manifest shipped with veo, never env values.
export async function downloadMediaAsset({
  asset, release, destination, signal, fetchImpl = fetch, timeoutMs = DOWNLOAD_TIMEOUT_MS,
}) {
  validateAsset(asset);
  if (!/^b\d+\.\d+(?:\.\d+)?$/.test(release)) throw new Error('Invalid pinned media release.');
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > DOWNLOAD_TIMEOUT_MS) throw new Error('Invalid media download timeout.');
  if (asset.gzip) validateAsset(asset.gzip);
  signal?.throwIfAborted();
  const deadline = new AbortController();
  const timer = setTimeout(() => deadline.abort(new DOMException('Media download timed out.', 'TimeoutError')), timeoutMs);
  const downloadSignal = AbortSignal.any([deadline.signal, ...(signal ? [signal] : [])]);
  const transferAsset = asset.gzip || asset;
  const compressed = asset.gzip ? `${destination}.gz` : undefined;
  const transferDestination = compressed || destination;
  let createdTransfer = false;
  let createdDestination = false;
  try {
    const response = await mediaResponse(`${MEDIA_ASSET_BASE}/${release}/${transferAsset.name}`, { signal: downloadSignal, fetchImpl });
    const length = response.headers.get('content-length');
    if (length !== null && (!/^\d+$/.test(length) || Number(length) !== transferAsset.size)) {
      await response.body.cancel?.().catch(() => {});
      throw new Error(`Unexpected content length for ${transferAsset.name}: does not match pinned byte count.`);
    }
    const transfer = createWriteStream(transferDestination, { flags: 'wx', mode: 0o600 });
    transfer.once('open', () => { createdTransfer = true; });
    const body = typeof response.body.getReader === 'function' ? Readable.fromWeb(response.body) : Readable.from(response.body);
    await pipeline(body, integrityStream(transferAsset), transfer, { signal: downloadSignal });
    if (asset.gzip) {
      const output = createWriteStream(destination, { flags: 'wx', mode: 0o600 });
      output.once('open', () => { createdDestination = true; });
      await pipeline(createReadStream(compressed), createGunzip(), integrityStream(asset), output, { signal: downloadSignal });
    }
    downloadSignal.throwIfAborted();
    return destination;
  } catch (error) {
    if (createdDestination) await rm(destination, { force: true });
    if (createdTransfer) await rm(transferDestination, { force: true });
    downloadSignal.throwIfAborted();
    throw error;
  } finally {
    clearTimeout(timer);
    if (compressed && createdTransfer) await rm(compressed, { force: true });
  }
}

async function verifyMediaFile(file, asset, signal) {
  signal?.throwIfAborted();
  const info = await lstat(file);
  if (!info.isFile() || info.isSymbolicLink() || info.size !== asset.size) return false;
  const handle = await open(file, constants.O_RDONLY | (constants.O_NOFOLLOW || 0));
  try {
    const opened = await handle.stat();
    if (!opened.isFile() || opened.size !== asset.size || opened.dev !== info.dev || opened.ino !== info.ino) return false;
    await pipeline(handle.createReadStream({ autoClose: false }), integrityStream(asset), new Writable({ write(_chunk, _encoding, callback) { callback(); } }), { signal });
    const after = await lstat(file);
    return after.isFile() && !after.isSymbolicLink() && after.dev === opened.dev && after.ino === opened.ino && after.size === opened.size && after.mtimeMs === opened.mtimeMs;
  } finally {
    await handle.close();
  }
}

export async function verifiedMediaTools({ directory, platform = process.platform, arch = process.arch, signal }) {
  const plan = mediaPackagePlan(platform, arch);
  if (!plan) return false;
  const suffix = platform === 'win32' ? '.exe' : '';
  try {
    for (const tool of ['ffmpeg', 'ffprobe']) {
      if (!await verifyMediaFile(path.join(directory, `${tool}${suffix}`), plan[tool], signal)) return false;
    }
    return true;
  } catch {
    signal?.throwIfAborted();
    return false;
  }
}

export function mediaProbeEnvironment(env = process.env) {
  // Native version probes do not need any uploader configuration or secrets.
  return Object.fromEntries(Object.entries(env).filter(([name]) => !/^(?:SMOLUP|SMOP|SMUP)_/i.test(name)));
}

export async function installMediaTools({
  directory, stage, signal, status = () => {}, platform = process.platform, arch = process.arch,
  env = process.env, run = runSetup, fetchImpl = fetch,
}) {
  const plan = mediaPackagePlan(platform, arch);
  if (!plan) throw new Error(`Automatic FFmpeg setup is unavailable for ${platform}/${arch}. Set VEO_FFMPEG_PATH to a directory containing ffmpeg and ffprobe.`);
  signal?.throwIfAborted();
  const setupSignal = AbortSignal.any([AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS), ...(signal ? [signal] : [])]);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const temporary = await mkdtemp(path.join(directory, '.media-setup-'));
  const suffix = platform === 'win32' ? '.exe' : '';
  try {
    if (process.platform !== 'win32') await chmod(temporary, 0o700);
    status('Downloading FFmpeg and FFprobe into the veo cache (first use)…');
    const files = {
      license: path.join(temporary, 'FFmpeg.LICENSE'),
      readme: path.join(temporary, 'FFmpeg.README'),
      ffmpeg: path.join(temporary, `ffmpeg${suffix}`),
      ffprobe: path.join(temporary, `ffprobe${suffix}`),
    };
    for (const name of ['license', 'readme', 'ffmpeg', 'ffprobe']) {
      await downloadMediaAsset({ asset: plan[name], release: plan.release, destination: files[name], signal: setupSignal, fetchImpl });
    }
    // Verify the whole pair before either downloaded program may execute.
    if (!await verifiedMediaTools({ directory: temporary, platform, arch, signal: setupSignal })) throw new Error('Downloaded media tools failed pinned integrity verification.');
    for (const name of ['license', 'readme']) {
      if (!await verifyMediaFile(files[name], plan[name], setupSignal)) throw new Error('Downloaded media notices failed pinned integrity verification.');
    }
    for (const name of ['ffmpeg', 'ffprobe']) {
      if (process.platform !== 'win32') await chmod(files[name], 0o700);
      await run(files[name], ['-version'], { signal: setupSignal, env: mediaProbeEnvironment(env), timeoutMs: 15_000 });
    }
    for (const name of ['ffmpeg', 'ffprobe', 'license', 'readme']) {
      await stage(files[name], path.join(directory, path.basename(files[name])), setupSignal);
    }
    if (!await verifiedMediaTools({ directory, platform, arch, signal: setupSignal })) throw new Error('Staged media tools failed pinned integrity verification.');
    for (const name of ['license', 'readme']) {
      if (!await verifyMediaFile(path.join(directory, path.basename(files[name])), plan[name], setupSignal)) throw new Error('Staged media notices failed pinned integrity verification.');
    }
    setupSignal.throwIfAborted();
    return directory;
  } catch (cause) {
    signal?.throwIfAborted();
    throw new Error(`Automatic FFmpeg setup failed: ${cause.message}. Retry with veo doctor fix or set VEO_FFMPEG_PATH to installed tools.`, { cause });
  } finally { await rm(temporary, { recursive: true, force: true }); }
}
