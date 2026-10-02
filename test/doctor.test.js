import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { collectChecks, doctorMain, findProbeVersion, formatReport, summarize, DOCTOR_HELP } from '../src/doctor.js';

function io() {
  let stdout = '';
  let stderr = '';
  return {
    stdout: { write: text => { stdout += text; } },
    stderr: { write: text => { stderr += text; } },
    get stdoutText() { return stdout; },
    get stderrText() { return stderr; },
  };
}

// A backend that is fully present, so only the injected bits decide the outcome.
function healthyBackend(directory) {
  return {
    release: '2026.08.19',
    directory,
    platform: 'win32/x64',
    asset: 'yt-dlp.exe',
    ytDlp: { source: 'managed', path: path.join(directory, 'yt-dlp.exe'), present: true, verified: true },
    ffmpeg: { source: 'cache', path: path.join(directory, 'ffmpeg.exe'), present: true, verified: true },
    ffprobe: { source: 'cache', path: path.join(directory, 'ffprobe.exe'), present: true, verified: true },
    errors: [],
  };
}

async function deps(overrides = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-doctor-'));
  const streams = io();
  return {
    directory,
    streams,
    options: {
      version: '1.0.2',
      offline: true,
      cwd: directory,
      env: {},
      stdout: streams.stdout,
      stderr: streams.stderr,
      inspect: async () => healthyBackend(directory),
      find: async () => undefined,
      runProbe: async () => '2026.08.19\n',
      fetchLatest: async () => '1.0.2',
      ...overrides,
    },
  };
}

test('probe versions are read from tool output', () => {
  assert.equal(findProbeVersion('2026.08.19\n'), '2026.08.19');
  // Nightly builds carry their build stamp; report it verbatim.
  assert.equal(findProbeVersion('2025.01.15.232704'), '2025.01.15.232704');
  assert.equal(findProbeVersion('ffmpeg version 7.0.2'), undefined);
});

test('doctor never executes media from a cache that failed integrity verification', async () => {
  const fixture = await deps();
  const probes = [];
  try {
    const report = healthyBackend(fixture.directory);
    report.ffmpeg.verified = false;
    report.ffprobe.present = false;
    report.ffprobe.verified = false;
    const checks = await collectChecks({ ...fixture.options,
      inspect: async () => report, runProbe: async file => { probes.push(file); return '2026.08.19'; } });
    assert.deepEqual(probes, [report.ytDlp.path]);
    for (const name of ['ffmpeg', 'ffprobe']) {
      const check = checks.find(check => check.label === name);
      assert.equal(check.level, 'fail');
      assert.match(check.detail, /SHA-256 verification failed/);
    }
  } finally { await rm(fixture.directory, { recursive: true, force: true }); }
});

test('summaries and exit codes follow failures, not warnings', () => {
  assert.deepEqual(summarize([{ level: 'ok' }, { level: 'warn' }]), { failed: 0, warned: 1, exitCode: 0 });
  assert.deepEqual(summarize([{ level: 'warn' }, { level: 'fail' }, { level: 'fail' }]), { failed: 2, warned: 1, exitCode: 1 });
  const report = formatReport({ version: '1.2.3', checks: [{ level: 'ok', label: 'Node.js', detail: 'v22' }], summary: summarize([]) });
  assert.match(report, /^veo 1\.2\.3 doctor\n\n {2}ok {4}Node\.js +v22\n\nNo problems found\.\n$/);
});

test('a healthy setup reports no problems', async () => {
  const { directory, streams, options } = await deps();
  try {
    const checks = await collectChecks(options);
    assert.equal(summarize(checks).failed, 0);
    assert.equal(checks.find(check => check.label === 'System FFmpeg').level, 'ok');
    assert.ok(checks.some(check => check.label === 'yt-dlp' && check.detail.includes('2026.08.19')));
    assert.equal(await doctorMain(['--offline'], options), 0);
    assert.match(streams.stdoutText, /No problems found/);
    assert.equal(streams.stderrText, '');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('Termux system yt-dlp is probed without pinned hashes and missing tools show pkg commands', async () => {
  const dir = await deps({ platform: 'android', arch: 'arm64', hasEjs: async () => true });
  try {
    const report = { ...healthyBackend(dir.directory), asset: undefined,
      ytDlp: { source: 'system', path: '/termux/bin/yt-dlp', present: true, verified: false } };
    const checks = await collectChecks({ ...dir.options, inspect: async () => report });
    assert.equal(summarize(checks).failed, 0);
    assert.match(checks.find(check => check.label === 'yt-dlp').detail, /system installation, not pinned/);
    report.ytDlp.present = false;
    report.ffmpeg.present = false;
    report.ffprobe.present = false;
    const missing = await collectChecks({ ...dir.options, inspect: async () => report });
    assert.equal(summarize(missing).failed, 3);
    assert.match(missing.find(check => check.label === 'yt-dlp').detail, /pkg install python-yt-dlp/);
    assert.ok(!missing.some(check => /bundled tools|downloaded on first/.test(check.detail)));
  } finally { await rm(dir.directory, { recursive: true, force: true }); }
});

test('missing backend pieces and unusable tools fail the check', async () => {
  const missing = await deps();
  try {
    const checks = await collectChecks({
      ...missing.options,
      inspect: async () => ({
        release: '2026.08.19',
        directory: missing.directory,
        asset: undefined,
        ytDlp: { source: 'invalid', present: false, verified: false },
        ffmpeg: { source: 'missing', present: false },
        ffprobe: { source: 'missing', present: false },
        errors: ['VEO_YT_DLP_PATH must be a non-empty filesystem path.'],
      }),
    });
    const summary = summarize(checks);
    assert.equal(summary.failed, 4); // platform, env, both media tools
    assert.ok(checks.some(check => check.label === 'Platform' && check.level === 'fail'));
    assert.ok(checks.some(check => check.label === 'Environment'));
  } finally { await rm(missing.directory, { recursive: true, force: true }); }

  const broken = await deps();
  try {
    const checks = await collectChecks({ ...broken.options, runProbe: async () => { throw new Error('EPERM'); } });
    assert.ok(checks.some(check => check.label === 'yt-dlp' && check.level === 'fail' && /could not run/.test(check.detail)));
  } finally { await rm(broken.directory, { recursive: true, force: true }); }
});

test('a pending yt-dlp download is a warning, not a failure', async () => {
  const pending = await deps();
  try {
    const checks = await collectChecks({
      ...pending.options,
      inspect: async () => ({ ...healthyBackend(pending.directory), ytDlp: { source: 'managed', path: path.join(pending.directory, 'yt-dlp.exe'), present: false, verified: false } }),
    });
    assert.equal(summarize(checks).failed, 0);
    assert.ok(checks.some(check => check.label === 'yt-dlp' && check.level === 'warn' && /not cached/.test(check.detail)));
  } finally { await rm(pending.directory, { recursive: true, force: true }); }
});

test('network warnings never fail the run, and outdated versions warn', async () => {
  const dir = await deps();
  try {
    const checks = await collectChecks({
      ...dir.options,
      offline: false,
      fetchLatest: async () => '2.0.0',
      fetchImpl: async () => { throw new Error('getaddrinfo ENOTFOUND'); },
    });
    assert.equal(summarize(checks).failed, 0);
    assert.ok(checks.some(check => check.label === 'veo version' && check.level === 'warn' && /2\.0\.0 is available/.test(check.detail)));
    assert.ok(checks.some(check => check.label === 'GitHub' && check.level === 'warn'));
  } finally { await rm(dir.directory, { recursive: true, force: true }); }
});

test('stale download-history records warn, a stray history file fails', async () => {
  const dir = await deps();
  try {
    assert.ok((await collectChecks(dir.options)).some(check => check.label === 'Download history' && check.level === 'ok'));
    const historyDir = path.join(dir.directory, '.veo-history');
    await mkdir(historyDir, { recursive: true });
    assert.match((await collectChecks(dir.options)).find(check => check.label === 'Download history').detail, /empty record folder/);
    await writeFile(path.join(historyDir, 'dead.json'), JSON.stringify({ version: 1, files: [path.join(dir.directory, 'gone.mp4')] }));
    const live = path.join(dir.directory, 'kept.mp4');
    await writeFile(live, 'x');
    await writeFile(path.join(historyDir, 'live.json'), JSON.stringify({ version: 1, files: [live] }));
    const checks = await collectChecks(dir.options);
    const warned = checks.find(check => check.label === 'Download history');
    assert.equal(warned.level, 'warn');
    assert.match(warned.detail, /1 of 2 records reference missing files/);
    assert.equal(summarize(checks).failed, 0);
    await rm(historyDir, { recursive: true, force: true });
    await writeFile(historyDir, 'not a directory');
    assert.ok((await collectChecks(dir.options)).some(check => check.label === 'Download history' && check.level === 'fail'));
  } finally { await rm(dir.directory, { recursive: true, force: true }); }
});

test('doctor help and argument handling', async () => {
  const dir = await deps();
  try {
    const streams = io();
    assert.equal(await doctorMain(['--help'], { ...dir.options, stdout: streams.stdout, stderr: streams.stderr }), 0);
    assert.equal(streams.stdoutText, DOCTOR_HELP);
    await assert.rejects(doctorMain(['--nope'], dir.options), /Unknown option for veo doctor/);
    await assert.rejects(doctorMain(['-o'], dir.options), /--output requires a directory/);
  } finally { await rm(dir.directory, { recursive: true, force: true }); }
});

test('doctor fix forwards offline mode, rechecks repairs and reports repair failures', async () => {
  const dir = await deps();
  let repaired = false;
  try {
    const options = { ...dir.options,
      repairBackend: async ({ offline }) => { assert.equal(offline, true); repaired = true; },
      inspect: async () => { assert.equal(repaired, true); return healthyBackend(dir.directory); },
    };
    assert.equal(await doctorMain(['fix', '--offline'], options), 0);
    assert.match(dir.streams.stdoutText, /Checking setup after repairs/);
    assert.equal(await doctorMain(['fix', '--offline'], { ...options, repairBackend: async () => { throw new Error('repair failed'); } }), 1);
    assert.match(dir.streams.stdoutText, /repair failed/);
  } finally { await rm(dir.directory, { recursive: true, force: true }); }
});
