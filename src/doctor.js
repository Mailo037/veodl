import { commandOutput } from './output.js';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { inspectBackend, findOnPath, resolveBackend, TERMUX_SETUP } from './backend.js';
import { loadConfig } from './config.js';
import { cacheBase } from './paths.js';
import { fetchLatestVersion, compareVersions, packageVersion, defaultRegistry, veoCacheBase } from './updater.js';
import { cleanText, readableError } from './utils.js';
import { hasTermuxEjs } from './tool-setup.js';

export const DOCTOR_HELP = `veo doctor - diagnose the local setup

Usage:
  veo doctor [options]
  veo doctor fix [options]

Options:
  -o, --output <path>   Output directory to check (default: current directory)
  --offline            Skip network checks
  -h, --help            Show help

Checks Node.js, the output directory, the backend cache, yt-dlp, FFmpeg/FFprobe,
reachability of the registry and the yt-dlp release host, leftover partial
downloads, and stale duplicate-detection records. Downloads no backend and
changes nothing but its own probe files and
the backend cache directory. The fix command restores managed tools (downloads
yt-dlp if needed unless --offline), creates the requested output directory, and
checks again. It does not change PATH, overrides or config values.
Missing desktop media tools are downloaded into veo's cache automatically.
On Android/Termux, fix installs missing tools with: ${TERMUX_SETUP} -y
With --offline no tools are downloaded and no package manager is run.
Exit status is 0 when no check fails, 1 otherwise.
`;

const LABEL_WIDTH = 18;

export function findProbeVersion(text, pattern = /\d{4}\.\d{2}\.\d{2}(?:\.\d+)?/) {
  return cleanText(String(text)).match(pattern)?.[0];
}

// Runs a trusted backend binary with fixed arguments; never uses a shell.
export function probe(executable, args, { timeoutMs = 15_000 } = {}) {
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(executable, args, { shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    } catch (error) {
      reject(error);
      return;
    }
    let output = '';
    let errors = '';
    const timer = setTimeout(() => { child.kill(); reject(new Error('timed out')); }, timeoutMs);
    timer.unref?.();
    child.stdout.on('data', data => { if (output.length < 4096) output += data; });
    child.stderr.on('data', data => { if (errors.length < 4096) errors += data; });
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.on('close', code => {
      clearTimeout(timer);
      if (code === 0) resolve(output || errors);
      else reject(new Error(errors.trim() || `${path.basename(executable)} exited with code ${code}`));
    });
  });
}

async function writable(directory) {
  try {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const probeDirectory = await mkdtemp(path.join(directory, '.veo-write-'));
    await writeFile(path.join(probeDirectory, 'probe'), 'veo');
    await rm(probeDirectory, { recursive: true, force: true });
    return { ok: true };
  } catch (error) {
    return { ok: false, reason: readableError(error) };
  }
}

// The nearest existing ancestor decides whether a not-yet-created output
// directory will be creatable, without creating it as a side effect.
async function nearestExisting(directory) {
  let current = path.resolve(directory);
  for (;;) {
    try {
      if ((await stat(current)).isDirectory()) return current;
    } catch { /* keep walking up */ }
    const parent = path.dirname(current);
    if (parent === current) return undefined;
    current = parent;
  }
}

export function summarize(checks) {
  const failed = checks.filter(check => check.level === 'fail').length;
  const warned = checks.filter(check => check.level === 'warn').length;
  return { failed, warned, exitCode: failed ? 1 : 0 };
}

export function formatReport({ version, checks, summary }) {
  const lines = [`veo ${version} doctor`, ''];
  for (const check of checks) lines.push(`  ${check.level.padEnd(4)}  ${check.label.padEnd(LABEL_WIDTH)}${check.detail}`);
  lines.push('');
  lines.push(summary.failed
    ? `${summary.failed} problem${summary.failed === 1 ? '' : 's'} found${summary.warned ? `, ${summary.warned} warning${summary.warned === 1 ? '' : 's'}` : ''}.`
    : summary.warned ? `No problems found, ${summary.warned} warning${summary.warned === 1 ? '' : 's'}.` : 'No problems found.');
  return `${lines.join('\n')}\n`;
}

export async function collectChecks({
  version,
  platform = process.platform,
  arch = process.arch,
  nodeVersion = process.versions.node,
  engines = '>=22',
  output,
  cwd = process.cwd(),
  env = process.env,
  offline = false,
  inspect = inspectBackend,
  find = findOnPath,
  loadConfigImpl,
  fetchImpl = fetch,
  runProbe = probe,
  hasEjs = hasTermuxEjs,
  registry = defaultRegistry(env),
  fetchLatest = fetchLatestVersion,
} = {}) {
  const checks = [];
  const push = (level, label, detail) => checks.push({ level, label, detail });

  const major = Number(String(nodeVersion).split('.')[0]);
  push(major >= 22 ? 'ok' : 'fail', 'Node.js', `v${nodeVersion} (veo requires ${engines.replace(/^>=\s*/, 'Node.js ')}${major >= 22 ? '' : ' — please upgrade'})`);

  const report = await inspect().catch(error => ({ errors: [readableError(error)], ytDlp: {}, ffmpeg: {}, ffprobe: {} }));
  const supported = platform === 'android' || Boolean(report.asset) || report.ytDlp?.source === 'override';
  push(supported ? 'ok' : 'fail', 'Platform', `${platform}/${arch}${platform === 'android' ? ' (Termux system tools)' : report.asset ? ` (yt-dlp ${report.asset})` : report.ytDlp?.source === 'override' ? ' (VEO_YT_DLP_PATH)' : ' — set VEO_YT_DLP_PATH to a trusted executable'}`);

  for (const message of report.errors || []) push('fail', 'Environment', message);

  const target = path.resolve(cwd, output || cwd);
  const existing = await nearestExisting(target);
  const outputAccess = existing ? await writable(existing) : { ok: false, reason: 'no existing parent directory' };
  const outputNote = existing === target ? '' : existing ? ` (will be created under ${existing})` : '';
  push(outputAccess.ok ? 'ok' : 'fail', 'Output directory', `${target}${outputNote}${outputAccess.ok ? '' : ` — ${outputAccess.reason}`}`);

  const cacheAccess = await writable(report.directory || veoCacheBase({ platform, env }));
  push(cacheAccess.ok ? 'ok' : 'fail', 'Backend cache', `${report.directory || veoCacheBase({ platform, env })}${cacheAccess.ok ? '' : ` — ${cacheAccess.reason}`}`);

  const yt = report.ytDlp || {};
  if (yt.present && !['override', 'system'].includes(yt.source) && !yt.verified) {
    push('fail', 'yt-dlp', `${yt.path} — SHA-256 verification failed; run veo doctor fix`);
  } else if (yt.present) {
    let detail = `${yt.path} (${yt.source === 'override' ? 'VEO_YT_DLP_PATH' : yt.source === 'system' ? 'system installation, not pinned by veo' : `release ${report.release}${yt.verified ? ', SHA-256 verified' : ''}`})`;
    try {
      const out = await runProbe(yt.path, ['--version']);
      const probed = findProbeVersion(out);
      detail = `${probed ? `version ${probed}, ` : ''}${detail}`;
      push('ok', 'yt-dlp', detail);
    } catch (error) {
      push('fail', 'yt-dlp', `${detail} — could not run it: ${readableError(error)}`);
    }
  } else if (platform === 'android') {
    push('fail', 'yt-dlp', `not found — run veo doctor fix to install automatically (Termux: ${TERMUX_SETUP})`);
  } else if (supported) {
    push('warn', 'yt-dlp', `not cached yet; downloaded on first download (release ${report.release ?? 'unknown'})`);
  }

  if (platform === 'android' && yt.source !== 'override') {
    const ejs = await hasEjs({ find, run: runProbe });
    push(ejs ? 'ok' : 'fail', 'YouTube JS', ejs ? 'yt-dlp-ejs installed; veo uses Node.js' : 'yt-dlp-ejs missing — run veo doctor fix to install automatically');
  }

  for (const name of ['ffmpeg', 'ffprobe']) {
    const tool = report[name] || {};
    if (tool.source === 'cache' && tool.verified === false) {
      push('fail', name, `${tool.path} — SHA-256 verification failed; run veo doctor fix`);
      continue;
    }
    if (!tool.present) {
      push('fail', name, 'not found — run veo doctor fix to install automatically, or set VEO_FFMPEG_PATH to a directory containing ffmpeg and ffprobe');
      continue;
    }
    const source = tool.source === 'override' ? 'VEO_FFMPEG_PATH' : tool.source === 'cache' ? 'managed cache' : 'system installation';
    try {
      await runProbe(tool.path, ['-version']);
      push('ok', name, `${tool.path} (${source}${tool.verified ? ', SHA-256 verified' : ''})`);
    } catch (error) {
      push('fail', name, `${tool.path} (${source}) — could not run it: ${readableError(error)}`);
    }
  }

  // Only relevant when the bundled tools cannot serve this platform (e.g. Windows on ARM).
  const systemFfmpeg = await find(['ffmpeg']);
  const systemFfprobe = await find(['ffprobe']);
  const mediaReady = ['ffmpeg', 'ffprobe'].every(name => checks.some(check => check.label === name && check.level === 'ok'));
  push(systemFfmpeg && systemFfprobe || mediaReady ? 'ok' : 'warn', 'System FFmpeg',
    systemFfmpeg && systemFfprobe ? `${path.dirname(systemFfmpeg)} (fallback)` : mediaReady ? 'not required — the selected FFmpeg and FFprobe work' : platform === 'android' ? 'not on PATH — in Termux run: pkg install ffmpeg' : 'not on PATH — run veo doctor fix to prepare the bundled tools');

  const partials = await readdir(target, { withFileTypes: true }).catch(() => []);
  const leftover = partials.filter(entry => entry.isDirectory() && entry.name.startsWith('.veo-') && entry.name !== '.veo-history').map(entry => entry.name);
  if (leftover.length) push('warn', 'Partial data', `${leftover.length} legacy folder${leftover.length === 1 ? '' : 's'} in the output directory (${leftover.slice(0, 3).join(', ')}${leftover.length > 3 ? ', …' : ''}). These are not migrated to the local cache; inspect them before removing them.`);
  else push('ok', 'Partial data', 'no leftover download folders');
  const historyDir = path.join(target, '.veo-history');
  const historyInfo = await stat(historyDir).catch(() => null);
  if (!historyInfo) push('ok', 'Download history', 'no duplicate-detection records in the output directory');
  else if (!historyInfo.isDirectory()) push('fail', 'Download history', `${historyDir} is not a directory — remove or rename it; downloads cannot save their duplicate-detection records otherwise`);
  else {
    const records = (await readdir(historyDir).catch(() => [])).filter(name => name.endsWith('.json'));
    let stale = 0;
    for (const name of records) {
      let usable = false;
      try {
        const record = JSON.parse(await readFile(path.join(historyDir, name), 'utf8'));
        usable = Boolean(record?.files?.length && record.files.every(file => typeof file === 'string' && path.dirname(file) === target)
          && (await Promise.all(record.files.map(file => stat(file).then(info => info.isFile(), () => false)))).every(Boolean));
      } catch { usable = false; }
      if (!usable) stale++;
    }
    if (!records.length) push('ok', 'Download history', 'empty record folder; removed automatically on the next download into this folder');
    else if (stale) push('warn', 'Download history', `${stale} of ${records.length} record${records.length === 1 ? '' : 's'} reference${records.length === 1 ? 's' : ''} missing files; removed automatically on the next download into this folder`);
    else push('ok', 'Download history', `${records.length} record${records.length === 1 ? '' : 's'}, all files present`);
  }
  const localDownloads = path.join(cacheBase({ env }), 'downloads');
  const cachedDownloads = await readdir(localDownloads, { withFileTypes: true }).catch(() => []);
  const count = cachedDownloads.filter(entry => entry.isDirectory() && /^\.veo-part-[a-f0-9]{24}$/.test(entry.name)).length;
  push(count ? 'warn' : 'ok', 'Local downloads', `${localDownloads}: ${count} retained download folder(s). Failed transfers expire after 15 minutes and are cleaned on the next run; unfinished --resume downloads are kept.`);

  try {
    const loaded = await (loadConfigImpl || loadConfig)({ env });
    const keys = Object.keys(loaded.config || {});
    push('ok', 'Config', loaded.exists ? `${loaded.file} (${keys.length ? keys.join(', ') : 'no defaults set'})` : `${loaded.file} not present (optional)`);
    for (const warning of loaded.warnings || []) push('warn', 'Config', warning);
  } catch (error) {
    push('fail', 'Config', `${readableError(error)}`);
  }

  if (offline) {
    push('warn', 'Network', 'checks skipped (--offline)');
  } else {
    let latest = null;
    try {
      latest = await fetchLatest({ registry, fetchImpl, timeoutMs: 6000 });
      push('ok', 'npm registry', `${registry} reachable`);
    } catch (error) {
      push('warn', 'npm registry', `${registry} unreachable — ${readableError(error)}`);
    }
    try {
      const response = await fetchImpl('https://github.com/yt-dlp/yt-dlp/releases', { method: 'HEAD', signal: AbortSignal.timeout(6000) });
      await response.body?.cancel();
      push(response.ok || response.status < 500 ? 'ok' : 'warn', 'GitHub', `yt-dlp release host reachable (HTTP ${response.status})`);
    } catch (error) {
      push('warn', 'GitHub', `yt-dlp release host unreachable — ${readableError(error)} (set VEO_YT_DLP_PATH to use a local backend)`);
    }
    if (latest) {
      const newer = compareVersions(latest, version) > 0;
      push(newer ? 'warn' : 'ok', 'veo version', newer ? `${version} — ${latest} is available (run: veo up)` : `${version} is the latest release`);
    }
  }
  return checks;
}

export async function doctorMain(args = [], {
  stdout = process.stdout,
  stderr = process.stderr,
  env = process.env,
  cwd = process.cwd(),
  registry = defaultRegistry(env),
  ...deps
} = {}) {
  [args, stdout, stderr] = commandOutput(args, stdout, stderr);
  let offline = false;
  let output;
  const fix = args[0] === 'fix';
  if (fix) args = args.slice(1);
  for (let index = 0; index < args.length; index++) {
    const token = args[index];
    if (token === '--offline') offline = true;
    else if (token === '-h' || token === '--help') { stdout.write(DOCTOR_HELP); return 0; }
    else if (token === '-o' || token === '--output') {
      output = args[++index];
      if (output === undefined) throw new Error('--output requires a directory. Run veo doctor --help for usage.');
    } else throw new Error(`Unknown option for veo doctor: ${cleanText(token)}. Run veo doctor --help for usage.`);
  }
  const version = deps.version ?? await packageVersion();
  const repairs = [];
  if (fix) {
    stdout.write('Repairing local setup…\n');
    try {
      await (deps.repairBackend || resolveBackend)({ offline, onStatus: message => stdout.write(`${cleanText(message)}\n`) });
      stdout.write('Backend tools are ready.\n');
    } catch (error) {
      repairs.push({ level: 'fail', label: 'Repair tools', detail: readableError(error) });
    }
    if (output) {
      try { await mkdir(path.resolve(cwd, output), { recursive: true }); }
      catch (error) { repairs.push({ level: 'fail', label: 'Repair output', detail: readableError(error) }); }
    }
    stdout.write('Checking setup after repairs…\n\n');
  }
  const checks = await collectChecks({ version, cwd, env, offline, output, registry, ...deps });
  checks.push(...repairs);
  const summary = summarize(checks);
  stdout.write(formatReport({ version, checks, summary }));
  if (summary.failed) stderr.write('veo: some checks failed. See the report above.\n');
  return summary.exitCode;
}
