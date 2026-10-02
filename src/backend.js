import { createHash, randomUUID } from 'node:crypto';
import { createReadStream, createWriteStream, constants } from 'node:fs';
import { access, chmod, copyFile, lstat, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { cacheBase } from './paths.js';
import { compareVersions } from './version.js';
import { hasTermuxEjs, installTermuxTools, installMediaTools, verifiedMediaTools, runSetup } from './tool-setup.js';

export const TERMUX_SETUP = 'pkg install python-yt-dlp yt-dlp-ejs ffmpeg';
export const RELEASE = '2026.08.19';
const RELEASE_HOST = 'https://github.com/yt-dlp/yt-dlp/releases/download';
const MAX_BYTES = 256 * 1024 * 1024;
const DOWNLOAD_TIMEOUT = 180_000;
const RELEASE_PATTERN = /^\d{4}\.\d{2}\.\d{2}(?:\.\d+)?$/;
// Official hashes fetched at development time; never trust a runtime checksum download.
// https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/SHA2-256SUMS
const HASHES = Object.freeze({
  'yt-dlp.exe': '66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a',
  'yt-dlp_arm64.exe': '05b438997bafc3affdfda9d041353c9d73e04dc842207254b655b0887c4445b0',
  'yt-dlp_x86.exe': 'a8f91bd41452506bc81ebd2f369b186fea0ee7075413ba00cef9fd346a0a5d0c',
  'yt-dlp_macos': '0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202',
  'yt-dlp_linux': '58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a',
  'yt-dlp_linux_aarch64': 'b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc',
  'yt-dlp_musllinux': 'f3dec9cfeaf304cec98290fe41c6ad465d4b747d302473559643e7af24929722',
  'yt-dlp_musllinux_aarch64': '17b164c4d258be92bb1ad146cb7c336b783aedb380814aabbcb7d52937f77e57',
});

export function selectAsset(platform = process.platform, arch = process.arch, musl = false) {
  if (platform === 'win32') return { x64: 'yt-dlp.exe', arm64: 'yt-dlp_arm64.exe', ia32: 'yt-dlp_x86.exe' }[arch];
  if (platform === 'darwin' && ['x64', 'arm64'].includes(arch)) return 'yt-dlp_macos';
  if (platform === 'linux' && ['x64', 'arm64'].includes(arch)) return `yt-dlp_${musl ? 'musl' : ''}linux${arch === 'arm64' ? '_aarch64' : ''}`;
}

function cacheDirectory(release = RELEASE) {
  return path.join(cacheBase(), 'backends', release, `${process.platform}-${process.arch}`);
}

export { cacheDirectory as backendCacheDirectory };

export function releaseUrl(release, asset) {
  return `${RELEASE_HOST}/${release}/${asset}`;
}

export function isValidRelease(value) {
  return typeof value === 'string' && RELEASE_PATTERN.test(value);
}

/** Path of the state file that records an explicitly installed newer backend. */
export function backendStateFile({ env = process.env } = {}) {
  return path.join(cacheBase({ env }), 'backend-override.json');
}

export async function readBackendOverride(stateFile = backendStateFile()) {
  try {
    const state = JSON.parse(await readFile(stateFile, 'utf8'));
    return state && typeof state === 'object' && !Array.isArray(state) ? state : null;
  } catch {
    return null;
  }
}

export async function writeBackendOverride(state, stateFile = backendStateFile()) {
  await mkdir(path.dirname(stateFile), { recursive: true, mode: 0o700 });
  const temp = `${stateFile}.${randomUUID()}.tmp`;
  try {
    await writeFile(temp, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    await rename(temp, stateFile);
  } finally {
    await rm(temp, { force: true });
  }
}

/** Returns true only when a recorded override was actually removed. */
export async function clearBackendOverride(stateFile = backendStateFile()) {
  try {
    await rm(stateFile);
    return true;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

/**
 * The explicitly installed backend only wins when it is newer than the pinned
 * release, matches this platform's asset, and its bytes still hash to the value
 * recorded at install time. Anything else silently falls back to the pinned
 * release, so a tampered or truncated cache never becomes the default.
 */
export async function activeOverride(asset, { signal, stateFile = backendStateFile(), matchesImpl = matches } = {}) {
  if (!asset) return null;
  const state = await readBackendOverride(stateFile);
  if (!state || state.asset !== asset) return null;
  if (!isValidRelease(state.release) || !/^[a-f0-9]{64}$/.test(String(state.sha256))) return null;
  if (compareVersions(state.release, RELEASE) <= 0) return null;
  const file = path.join(cacheDirectory(state.release), `yt-dlp${exeSuffix()}`);
  if (!await matchesImpl(file, state.sha256, signal)) return null;
  return { release: state.release, asset, sha256: state.sha256, path: file };
}

/**
 * Install a backend release that is newer than the pinned one. The expected
 * hash must come from that release's own checksum list; the caller is
 * responsible for the trust decision and for telling the user about it.
 */
export async function installBackend({ release, asset, sha256, signal, status = () => {} }) {
  if (!isValidRelease(release)) throw new Error(`Invalid yt-dlp release: ${release}`);
  if (!asset) throw new Error(`No standalone yt-dlp is available for ${process.platform}/${process.arch}.`);
  if (!/^[a-f0-9]{64}$/.test(String(sha256))) throw new Error('The release checksum is missing or malformed.');
  const directory = cacheDirectory(release);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const destination = await downloadBackend({ asset, release, expected: sha256, directory, signal, status, replace: true });
  return { release, asset, sha256, path: destination };
}

export function exeSuffix(platform = process.platform) {
  return platform === 'win32' ? '.exe' : '';
}

// Retained for callers that inspect the desktop media-tool platform matrix.
export function staticToolsSupported(platform = process.platform, arch = process.arch) {
  if (platform === 'win32') return ['x64', 'ia32', 'arm64'].includes(arch);
  if (platform === 'darwin') return ['x64', 'arm64'].includes(arch);
  if (platform === 'linux') return ['x64', 'ia32', 'arm', 'arm64'].includes(arch);
  return false;
}

async function isExecutable(file) {
  if (typeof file !== 'string' || !file) return false;
  try {
    if (!(await stat(file)).isFile()) return false;
    await access(file, process.platform === 'win32' ? constants.R_OK : constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Locate the first matching executable on PATH. Windows on ARM and minimal
 * containers cannot use the bundled static binaries, so a system ffmpeg is a
 * legitimate third source after the override and the managed cache.
 * Git for Windows and WinGet ship ffmpeg in directories such as these but do
 * not always extend the PATH that a Node process inherits.
 */
export function wellKnownMediaDirectories({ platform = process.platform, env = process.env } = {}) {
  if (platform === 'android') return env.PREFIX ? [path.join(env.PREFIX, 'bin')] : [];
  if (platform !== 'win32') return ['/opt/homebrew/bin', '/usr/local/bin', '/usr/bin', '/bin'];
  const local = env.LOCALAPPDATA || '';
  const programFiles = env.ProgramFiles || 'C:\\Program Files';
  return [
    local && path.join(local, 'Microsoft', 'WinGet', 'Links'),
    local && path.join(local, 'Microsoft', 'WindowsApps'),
    path.join(programFiles, 'ffmpeg', 'bin'),
    'C:\\ffmpeg\\bin',
    'C:\\ProgramData\\chocolatey\\bin',
  ].filter(Boolean);
}

export async function findOnPath(names, { platform = process.platform, env = process.env, directories } = {}) {
  const raw = platform === 'win32' ? env.PATH || env.Path || '' : env.PATH || '';
  const candidates = directories || [
    ...raw.split(platform === 'win32' ? ';' : ':').map(entry => entry.trim().replace(/^"(.*)"$/, '$1')),
    ...wellKnownMediaDirectories({ platform, env }),
  ];
  const suffixes = platform === 'win32' ? ['', '.exe', '.cmd'] : [''];
  for (const directory of candidates) {
    if (!path.isAbsolute(directory)) continue;
    for (const name of names) {
      for (const suffix of suffixes) {
        const candidate = path.join(directory, `${name}${suffix}`);
        if (await isExecutable(candidate)) return candidate;
      }
    }
  }
  return undefined;
}

function envPath(name) {
  if (!(name in process.env)) return undefined;
  const value = process.env[name];
  if (!value?.trim() || value.includes('\0')) throw new Error(`${name} must be a non-empty filesystem path.`);
  return path.resolve(value);
}

async function executable(file, label) {
  if (typeof file !== 'string' || !file) throw new Error(`${label} is unavailable on ${process.platform}/${process.arch}.`);
  try {
    if (!(await stat(file)).isFile()) throw new Error('not a regular file');
    await access(file, process.platform === 'win32' ? constants.R_OK : constants.X_OK);
  } catch (cause) {
    throw new Error(`${label} is missing or not executable: ${file}`, { cause });
  }
  return path.resolve(file);
}

async function sha256(file, signal) {
  signal?.throwIfAborted();
  const hash = createHash('sha256');
  let bytes = 0;
  for await (const chunk of createReadStream(file, { signal })) {
    bytes += chunk.length;
    if (bytes > MAX_BYTES) throw new Error(`Backend file exceeds ${MAX_BYTES / 1024 / 1024} MiB: ${file}`);
    hash.update(chunk);
  }
  return hash.digest('hex');
}

async function matches(file, expected, signal) {
  try {
    const info = await lstat(file);
    // Do not accept cached symlinks or special files.
    if (!info.isFile() || info.size > MAX_BYTES) return false;
    return await sha256(file, signal) === expected;
  } catch (error) {
    if (error.code === 'ENOENT') return false;
    throw error;
  }
}

async function publish(temp, destination, expected, signal) {
  signal?.throwIfAborted();
  if (process.platform !== 'win32') await chmod(temp, 0o755);
  try {
    await rename(temp, destination);
  } catch (error) {
    // Another process may have finished the same acquisition on Windows.
    if (['EEXIST', 'EPERM', 'EACCES'].includes(error.code)
      && await matches(destination, expected, signal)) return;
    // A directory at the destination is stale cache from an older layout.
    if (['EEXIST', 'EPERM', 'EACCES', 'ENOTEMPTY', 'EISDIR'].includes(error.code)) {
      const info = await lstat(destination).catch(() => undefined);
      if (info?.isDirectory()) {
        await rm(destination, { recursive: true, force: true });
        try { await rename(temp, destination); return; } catch { /* Fall through. */ }
      }
    }
    throw error;
  }
}

/**
 * Download one yt-dlp asset from its release and publish it only after the
 * SHA-256 of the received bytes matched `expected`. With `replace`, an already
 * present file is removed first, which is safe because the temp copy is fully
 * verified before it is published.
 */
async function downloadBackend({ asset, release, expected, directory, signal, status, replace = false }) {
  const destination = path.join(directory, `yt-dlp${exeSuffix()}`);
  const temp = `${destination}.${randomUUID()}.tmp`;
  const timeout = AbortSignal.timeout(DOWNLOAD_TIMEOUT);
  const downloadSignal = signal ? AbortSignal.any([signal, timeout]) : timeout;
  status(`Downloading yt-dlp ${release}…`);
  try {
    const response = await fetch(`${RELEASE_HOST}/${release}/${asset}`, { signal: downloadSignal });
    if (!response.ok || !response.body) {
      await response.body?.cancel();
      throw new Error(`yt-dlp download failed: HTTP ${response.status}.`);
    }
    if (Number(response.headers.get('content-length')) > MAX_BYTES) {
      await response.body.cancel();
      throw new Error('yt-dlp download exceeds the size limit.');
    }
    let bytes = 0;
    const hash = createHash('sha256');
    const verifier = new Transform({
      transform(chunk, encoding, callback) {
        bytes += chunk.length;
        if (bytes > MAX_BYTES) return callback(new Error('yt-dlp download exceeds the size limit.'));
        hash.update(chunk);
        callback(null, chunk);
      },
    });
    await pipeline(Readable.fromWeb(response.body), verifier,
      createWriteStream(temp, { flags: 'wx', mode: 0o600 }), { signal: downloadSignal });
    if (hash.digest('hex') !== expected) throw new Error('yt-dlp SHA-256 verification failed. The download was discarded.');
    if (replace) await rm(destination, { recursive: true, force: true });
    await publish(temp, destination, expected, downloadSignal);
    return destination;
  } catch (cause) {
    signal?.throwIfAborted();
    if (timeout.aborted) throw new Error('yt-dlp download timed out after 3 minutes. Please retry.', { cause });
    throw new Error(`Unable to prepare yt-dlp: ${cause.message} You can set VEO_YT_DLP_PATH to a trusted local executable.`, { cause });
  } finally {
    await rm(temp, { force: true });
  }
}

async function acquire(asset, directory, signal, status, statusDone = () => {}) {
  const destination = path.join(directory, `yt-dlp${exeSuffix()}`);
  const expected = HASHES[asset];
  status('Checking yt-dlp…');
  let cached;
  try {
    cached = await matches(destination, expected, signal);
    statusDone('Checking yt-dlp…', 'done');
  } catch (error) {
    if (!signal?.aborted && error?.name !== 'AbortError') statusDone('Checking yt-dlp…', 'failed');
    throw error;
  }
  if (cached) {
    if (process.platform !== 'win32') await chmod(destination, 0o755);
    return destination;
  }
  status(`Downloading yt-dlp ${RELEASE} (first use)…`);
  return downloadBackend({ asset, release: RELEASE, expected, directory, signal, status });
}

async function stage(source, destination, signal) {
  const expected = await sha256(source, signal);
  if (await matches(destination, expected, signal)) {
    if (process.platform !== 'win32') await chmod(destination, 0o755);
    return;
  }
  const temp = `${destination}.${randomUUID()}.tmp`;
  try {
    signal?.throwIfAborted();
    await copyFile(source, temp, constants.COPYFILE_EXCL);
    if (!await matches(temp, expected, signal)) throw new Error(`Backend copy verification failed: ${source}`);
    await publish(temp, destination, expected, signal);
  } finally {
    await rm(temp, { force: true });
  }
}

async function managedMediaTools({ signal, directory }) {
  if (!await verifiedMediaTools({ signal, directory })) {
    return 'the managed media tools are missing or failed SHA-256 verification';
  }
  if (process.platform !== 'win32') {
    for (const name of ['ffmpeg', 'ffprobe']) await chmod(path.join(directory, name), 0o755);
  }
  return null;
}

/**
 * Resolve a location that contains both ffmpeg and ffprobe for
 * --ffmpeg-location. Precedence: explicit override, verified managed cache,
 * then a system installation. Missing desktop tools use pinned downloads.
 */
export async function resolveMediaTools({ signal, status = () => {}, statusDone = () => {}, directory = cacheDirectory(), offline = false,
  find = findOnPath, managed = managedMediaTools, install = installMediaTools } = {}) {
  const preparing = 'Preparing ffmpeg and ffprobe…';
  let currentStatus;
  const report = message => { currentStatus = message; status(message); };
  const finish = outcome => {
    if (currentStatus !== preparing) report(preparing);
    statusDone(preparing, outcome);
  };
  report(preparing);
  try {
    const suffix = exeSuffix();
    const override = envPath('VEO_FFMPEG_PATH');
    if (override) {
      await executable(path.join(override, `ffmpeg${suffix}`), 'VEO_FFMPEG_PATH ffmpeg');
      await executable(path.join(override, `ffprobe${suffix}`), 'VEO_FFMPEG_PATH ffprobe');
      finish('done');
      report('Using VEO_FFMPEG_PATH override.');
      return override;
    }
    const problem = await managed({ signal, status: report, directory });
    if (!problem) { finish('done'); return directory; }
    const ffmpeg = await find(['ffmpeg']);
    const ffprobe = await find(['ffprobe']);
    if (!ffmpeg || !ffprobe) {
      if (!offline) {
        const location = await install({ signal, status: report, directory, find, stage });
        finish('done');
        return location;
      }
      const missing = [!ffmpeg && 'ffmpeg', !ffprobe && 'ffprobe'].filter(Boolean).join(' and ');
      throw new Error(`Cannot load media tools: ${problem}, and no ${missing} was found on PATH. `
        + 'Run veo doctor fix without --offline to install them automatically, or set VEO_FFMPEG_PATH to a directory containing both binaries.');
    }
    // Same directory: yt-dlp can use it in place instead of duplicating ~150 MB.
    if (path.dirname(ffmpeg) === path.dirname(ffprobe)) {
      finish('done');
      report(`Using the system ffmpeg in ${path.dirname(ffmpeg)}.`);
      return path.dirname(ffmpeg);
    }
    report('Copying system ffmpeg and ffprobe into the backend cache…');
    await mkdir(directory, { recursive: true, mode: 0o700 });
    await stage(ffmpeg, path.join(directory, `ffmpeg${suffix}`), signal);
    await stage(ffprobe, path.join(directory, `ffprobe${suffix}`), signal);
    finish('done');
    return directory;
  } catch (error) {
    if (!signal?.aborted && error?.name !== 'AbortError') finish('failed');
    throw error;
  }
}

/**
 * Report the state of every backend component without downloading, executing,
 * or changing anything. Used by `veo doctor`.
 */
export async function inspectBackend({ signal, platform = process.platform, arch = process.arch, find = findOnPath } = {}) {
  const suffix = exeSuffix();
  const directory = cacheDirectory();
  const musl = process.platform === 'linux' && !process.report.getReport().header.glibcVersionRuntime;
  const report = {
    release: RELEASE,
    directory,
    platform: `${platform}/${arch}`,
    asset: undefined,
    override: null,
    ytDlp: { source: 'none', present: false, verified: false },
    ffmpeg: { source: 'missing', present: false },
    ffprobe: { source: 'missing', present: false },
    errors: [],
  };
  try {
    const override = envPath('VEO_YT_DLP_PATH');
    if (override) {
      report.ytDlp = { source: 'override', path: override, present: await isExecutable(override), verified: false };
    } else if (platform === 'android') {
      const file = await find(['yt-dlp']);
      report.ytDlp = { source: 'system', path: file, present: Boolean(file), verified: false };
    } else {
      const asset = selectAsset(platform, arch, musl);
      report.asset = asset;
      if (!asset) {
        report.errors.push(`No standalone yt-dlp is published for ${report.platform}.`);
      } else {
        report.override = await activeOverride(asset, { signal });
        const file = report.override?.path || path.join(directory, `yt-dlp${suffix}`);
        const present = await isExecutable(file);
        report.ytDlp = {
          source: report.override ? 'installed' : 'managed',
          path: file,
          present,
          verified: present && await matches(file, report.override?.sha256 || HASHES[asset], signal),
        };
      }
    }
  } catch (error) {
    report.errors.push(error.message);
    report.ytDlp = { source: 'invalid', present: false, verified: false };
  }
  let override;
  try {
    override = envPath('VEO_FFMPEG_PATH');
  } catch (error) {
    report.errors.push(error.message);
  }
  const cacheVerified = platform !== 'android' && !override
    && await verifiedMediaTools({ directory, platform, arch, signal });
  for (const name of ['ffmpeg', 'ffprobe']) {
    const cached = path.join(directory, `${name}${suffix}`);
    let entry;
    if (override) {
      const file = path.join(override, `${name}${suffix}`);
      entry = { source: 'override', path: file, present: await isExecutable(file) };
    } else if (cacheVerified) {
      entry = { source: 'cache', path: cached, present: await isExecutable(cached), verified: true };
    } else {
      const found = await find([name]);
      entry = found ? { source: 'path', path: found, present: true, verified: false }
        : platform !== 'android' && await lstat(cached).then(() => true, () => false)
          ? { source: 'cache', path: cached, present: false, verified: false }
          : { source: 'missing', path: undefined, present: false, verified: false };
    }
    report[name] = entry;
  }
  return report;
}

/**
 * Resolve native tools, installing missing tools on first use (unless offline).
 * VEO_YT_DLP_PATH: trusted executable file; bypasses acquisition/pinned hash checks.
 * VEO_FFMPEG_PATH: directory containing both ffmpeg[.exe] and ffprobe[.exe].
 * Relative overrides resolve against cwd. Trusted system installations remain
 * available when the managed cache is absent or cannot serve this platform.
 * onStatus receives plain strings. Throws on cancellation or acquisition failure.
 */
export async function resolveBackend({ signal, onStatus, onStatusDone, offline = false, platform = process.platform, find = findOnPath,
  setupTermux = installTermuxTools, hasEjs = hasTermuxEjs, run = runSetup } = {}) {
  if (signal !== undefined && !(signal instanceof AbortSignal)) throw new TypeError('signal must be an AbortSignal.');
  if (onStatus !== undefined && typeof onStatus !== 'function') throw new TypeError('onStatus must be a function.');
  signal?.throwIfAborted();
  const status = onStatus || (() => {});
  const statusDone = onStatusDone || (() => {});
  const ytOverride = envPath('VEO_YT_DLP_PATH');
  if (platform === 'android') {
    const mediaOverride = envPath('VEO_FFMPEG_PATH');
    // Invalid explicit paths must fail before a package-manager mutation.
    if (ytOverride) await executable(ytOverride, 'VEO_YT_DLP_PATH');
    if (mediaOverride) {
      await executable(path.join(mediaOverride, 'ffmpeg'), 'VEO_FFMPEG_PATH ffmpeg');
      await executable(path.join(mediaOverride, 'ffprobe'), 'VEO_FFMPEG_PATH ffprobe');
    }
    const discover = async () => ({
      ytDlp: ytOverride || await find(['yt-dlp']),
      ffmpeg: mediaOverride ? path.join(mediaOverride, 'ffmpeg') : await find(['ffmpeg']),
      ffprobe: mediaOverride ? path.join(mediaOverride, 'ffprobe') : await find(['ffprobe']),
      ejs: Boolean(ytOverride) || await hasEjs({ find, signal, run }),
    });
    let tools = await discover();
    const ready = value => value.ytDlp && value.ffmpeg && value.ffprobe && value.ejs;
    if (!ready(tools)) {
      if (offline) throw new Error(`Termux tools are missing (yt-dlp, FFmpeg/FFprobe or JavaScript support). Run veo doctor fix without --offline, or: ${TERMUX_SETUP}`);
      await setupTermux({ find, signal, status, run });
      tools = await discover();
      if (!ready(tools)) throw new Error(`Termux setup finished but tools are still missing. Check PATH and run: ${TERMUX_SETUP}`);
      for (const [file, args] of [[tools.ytDlp, ['--version']], [tools.ffmpeg, ['-version']], [tools.ffprobe, ['-version']]]) {
        await run(file, args, { signal, timeoutMs: 15_000 });
      }
    }
    const { ytDlp, ffmpeg, ffprobe } = tools;
    await executable(ffmpeg, 'ffmpeg');
    await executable(ffprobe, 'ffprobe');
    if (path.dirname(ffmpeg) !== path.dirname(ffprobe)) throw new Error('Set VEO_FFMPEG_PATH to a directory containing both ffmpeg and ffprobe.');
    status(ytOverride ? 'Using VEO_YT_DLP_PATH override.' : 'Using system yt-dlp (Termux package; not pinned by veo).');
    signal?.throwIfAborted();
    return { ytDlp, ffmpegLocation: path.dirname(ffmpeg) };
  }
  // Diagnostic reports expose glibc when linked against it; no shell probe needed.
  const musl = process.platform === 'linux' && !process.report.getReport().header.glibcVersionRuntime;
  const asset = ytOverride ? undefined : selectAsset(process.platform, process.arch, musl);
  if (!ytOverride && !asset) throw new Error(`No standalone yt-dlp is available for ${process.platform}/${process.arch}. Set VEO_YT_DLP_PATH to a trusted executable.`);
  // Validate overrides and dependencies before doing any network work.
  const ytDlp = ytOverride ? await executable(ytOverride, 'VEO_YT_DLP_PATH') : undefined;
  // An explicitly installed newer backend takes precedence over the pinned one.
  // Media tools stay in the pinned release directory so they are never duplicated.
  const installed = ytDlp ? null : await activeOverride(asset, { signal });
  const directory = cacheDirectory();
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const ffmpegLocation = await resolveMediaTools({ signal, status, statusDone, directory, offline });
  if (ytDlp) status('Using VEO_YT_DLP_PATH override.');
  else if (installed) status(`Using the installed yt-dlp ${installed.release}.`);
  if (offline && !ytDlp && !installed && !await matches(path.join(directory, `yt-dlp${exeSuffix()}`), HASHES[asset], signal)) {
    throw new Error('yt-dlp is missing or damaged. Run veo doctor fix without --offline to download it.');
  }
  const backend = ytDlp || installed?.path || await acquire(asset, directory, signal, status, statusDone);
  signal?.throwIfAborted();
  return { ytDlp: backend, ffmpegLocation };
}
