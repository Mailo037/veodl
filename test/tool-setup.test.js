import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, chmod, readdir, rm } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { resolveBackend, resolveMediaTools } from '../src/backend.js';
import { createReporter } from '../src/progress.js';
import { runSetup, installTermuxTools, installMediaTools, mediaPackagePlan, mediaProbeEnvironment } from '../src/tool-setup.js';

const status = () => {};

test('fresh Termux setup installs once, rechecks tools and skips installation on later runs', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-termux-test-'));
  let installed = false;
  let installs = 0;
  const probes = [];
  try {
    const find = async names => installed ? path.join(directory, names[0]) : undefined;
    const options = { platform: 'android', find, hasEjs: async () => installed,
      setupTermux: async () => {
        installs++;
        for (const tool of ['yt-dlp', 'ffmpeg', 'ffprobe']) {
          const file = path.join(directory, tool);
          await writeFile(file, 'fixture');
          await chmod(file, 0o755);
        }
        installed = true;
      },
      run: async (command, args) => { probes.push([path.basename(command), args]); },
    };
    assert.equal((await resolveBackend(options)).ffmpegLocation, directory);
    assert.deepEqual(probes, [['yt-dlp', ['--version']], ['ffmpeg', ['-version']], ['ffprobe', ['-version']]]);
    await resolveBackend(options);
    assert.equal(installs, 1);
    installed = false;
    await assert.rejects(resolveBackend({ ...options, offline: true }), /without --offline/);
    assert.equal(installs, 1);
    await assert.rejects(resolveBackend({ ...options, setupTermux: async () => {} }), /still missing/);
    await assert.rejects(resolveBackend({ ...options, setupTermux: async () => { throw new Error('repository unavailable'); } }), /repository unavailable/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('Termux pkg uses fixed noninteractive arguments and concurrent callers share installation', async () => {
  const calls = [];
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const options = { find: async () => '/termux/bin/pkg', status, env: {},
    run: async (...args) => { calls.push(args); await gate; },
  };
  const first = installTermuxTools(options);
  const second = installTermuxTools(options);
  release();
  await Promise.all([first, second]);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].slice(0, 2), ['/termux/bin/pkg', ['install', '-y', 'python-yt-dlp', 'yt-dlp-ejs', 'ffmpeg']]);
  assert.equal(calls[0][2].env.DEBIAN_FRONTEND, 'noninteractive');
  await installTermuxTools({ ...options, env: { TERMUX_APP_PACKAGE_MANAGER: 'pacman' } });
  assert.equal(calls[1][1][1], '--noconfirm');
  await assert.rejects(installTermuxTools({ ...options, run: async () => { throw new Error('repository unavailable'); } }), /Automatic Termux setup failed/);
  await installTermuxTools(options); // failed attempts must not poison retries
});

test('desktop missing-media fallback obeys offline, cache and system-tool precedence', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-media-test-'));
  let installs = 0;
  const options = { directory, managed: async () => 'no bundled tools', find: async () => undefined,
    install: async () => { installs++; return directory; },
  };
  try {
    await assert.rejects(resolveMediaTools({ ...options, offline: true }), /without --offline/);
    assert.equal(installs, 0);
    assert.equal(await resolveMediaTools(options), directory);
    assert.equal(installs, 1);
    const systemDirectory = `${directory}-system`;
    await resolveMediaTools({ ...options, find: async names => path.join(systemDirectory, names[0]) });
    assert.equal(installs, 1);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('media-tool preparation is reported before checking bundled tools', async () => {
  const events = [];
  await resolveMediaTools({ directory: '.', status: message => events.push(message),
    managed: async () => { events.push('managed'); return null; } });
  assert.deepEqual(events, ['Preparing ffmpeg and ffprobe…', 'managed']);
});

test('ffmpeg preparation animates and finishes with a colored result', async () => {
  for (const outcome of ['done', 'failed']) {
    const output = { isTTY: true, columns: 80, text: '', write(value) { this.text += value; } };
    const reporter = createReporter(output, { setTitle() {}, env: {} });
    const options = { directory: '.', status: message => reporter.status(message),
      statusDone: (message, result) => reporter.finishStatus(message, result),
      managed: async () => { if (outcome === 'failed') throw new Error('media tools unavailable'); return null; } };
    if (outcome === 'failed') await assert.rejects(resolveMediaTools(options), /media tools unavailable/);
    else await resolveMediaTools(options);
    assert.match(output.text, /Preparing ffmpeg and ffprobe\./);
    assert.ok(output.text.endsWith(`Preparing ffmpeg and ffprobe: \x1b[0m\x1b[${outcome === 'done' ? 32 : 31}m${outcome}\x1b[0m\n`));
  }
});

test('desktop installer rejects an invalid asset before probing or staging and ignores mirror overrides', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-media-install-'));
  const calls = [];
  const staged = [];
  const urls = [];
  const options = { directory, platform: 'linux', arch: 'arm64', status, env: {},
    stage: async (...args) => staged.push(args), run: async (...args) => calls.push(args),
    fetchImpl: async (url, request) => {
      urls.push(url);
      assert.equal(request.headers?.Cookie, undefined);
      assert.equal(request.headers?.Authorization, undefined);
      return new Response('tampered download');
    },
  };
  const hostile = {
    FFMPEG_BIN: '/untrusted/ffmpeg', FFMPEG_BINARY_RELEASE: 'latest',
    FFMPEG_BINARIES_URL: 'https://attacker.invalid/media',
    npm_config_platform: 'darwin', npm_config_arch: 'x64',
  };
  const previous = Object.fromEntries(Object.keys(hostile).map(key => [key, process.env[key]]));
  try {
    Object.assign(process.env, hostile);
    await assert.rejects(installMediaTools({ ...options, env: hostile }), /size|SHA-256|checksum|length/i);
    assert.deepEqual(urls, ['https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/linux-arm64.LICENSE']);
    assert.deepEqual(calls, []);
    assert.deepEqual(staged, []);
    assert.deepEqual(await readdir(directory), []);
  } finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    await rm(directory, { recursive: true, force: true });
  }
});

test('a failed second media asset discards the verified first asset without executing anything', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-media-second-'));
  const license = await readFile(new URL('./fixtures/media.LICENSE', import.meta.url));
  const fetched = [];
  const calls = [];
  try {
    await assert.rejects(installMediaTools({ directory, platform: 'win32', arch: 'arm64', status,
      fetchImpl: async url => {
        fetched.push(url);
        return fetched.length === 1 ? new Response(license) : new Response('unavailable', { status: 503 });
      },
      run: async (...args) => calls.push(['probe', ...args]),
      stage: async (...args) => calls.push(['stage', ...args]),
    }), /503/);
    assert.deepEqual(fetched, [
      'https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/win32-x64.LICENSE',
      'https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/win32-x64.README',
    ]);
    assert.deepEqual(calls, []);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('a fresh nested media cache reaches acquisition and cleans a failed setup', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-media-fresh-'));
  const directory = path.join(root, 'missing', 'nested', 'media');
  const failure = new Error('fixture media network unavailable');
  const fetched = [];
  const calls = [];
  try {
    await assert.rejects(installMediaTools({ directory, platform: 'linux', arch: 'x64', status,
      fetchImpl: async url => { fetched.push(url); throw failure; },
      run: async (...args) => calls.push(['probe', ...args]),
      stage: async (...args) => calls.push(['stage', ...args]),
    }), error => {
      assert.match(error.message, /fixture media network unavailable/);
      assert.equal(error.cause, failure);
      return true;
    });
    assert.deepEqual(fetched, ['https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/linux-x64.LICENSE']);
    assert.deepEqual(calls, []);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('unsupported or pre-aborted media setup leaves a fresh cache uncreated', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-media-no-write-'));
  const controller = new AbortController();
  controller.abort(new Error('fixture media setup cancelled'));
  const calls = [];
  try {
    for (const [name, options, expected] of [
      ['unsupported', { platform: 'android', arch: 'arm64' }, /unavailable/],
      ['cancelled', { platform: 'linux', arch: 'x64', signal: controller.signal }, /fixture media setup cancelled/],
    ]) {
      const directory = path.join(root, name, 'media');
      await assert.rejects(installMediaTools({ directory, status, ...options,
        fetchImpl: async (...args) => calls.push(['fetch', ...args]),
        run: async (...args) => calls.push(['probe', ...args]),
        stage: async (...args) => calls.push(['stage', ...args]),
      }), expected);
      await assert.rejects(readdir(directory), { code: 'ENOENT' });
    }
    assert.deepEqual(calls, []);
    assert.deepEqual(await readdir(root), []);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('desktop media plans select fixed upstream targets, including Windows ARM64 emulation', () => {
  const targets = ['win32-x64', 'win32-ia32', 'darwin-x64', 'darwin-arm64', 'linux-x64', 'linux-ia32', 'linux-arm64', 'linux-arm'];
  for (const target of targets) {
    const [platform, arch] = target.split('-');
    const plan = mediaPackagePlan(platform, arch);
    assert.equal(plan.binaryArch, arch);
    assert.equal(plan.release, target === 'win32-ia32' ? 'b6.0' : 'b6.1.1');
    for (const name of ['ffmpeg', 'ffprobe']) {
      assert.equal(plan[name].name, `${name}-${target}`);
      assert.match(plan[name].sha256, /^[a-f0-9]{64}$/);
      assert.ok(Number.isSafeInteger(plan[name].size) && plan[name].size > 0);
      assert.equal(plan[name].gzip.name, `${name}-${target}.gz`);
      assert.match(plan[name].gzip.sha256, /^[a-f0-9]{64}$/);
      assert.ok(Number.isSafeInteger(plan[name].gzip.size) && plan[name].gzip.size > 0);
    }
    for (const name of ['license', 'readme']) {
      assert.match(plan[name].sha256, /^[a-f0-9]{64}$/);
      assert.ok(Number.isSafeInteger(plan[name].size) && plan[name].size > 0);
    }
  }
  assert.deepEqual(mediaPackagePlan('win32', 'arm64'), mediaPackagePlan('win32', 'x64'));
  assert.equal(mediaPackagePlan('android', 'arm64'), undefined);
  assert.equal(mediaPackagePlan('darwin', 'ia32'), undefined);
  assert.equal(mediaPackagePlan('linux', 'riscv64'), undefined);
});

test('media binary probe environment excludes uploader cookies and tokens', () => {
  const environment = { PATH: '/fixture/bin', HOME: '/fixture/home',
    SMOLUP_COOKIE: 'secret-a', SMOP_COOKIE: 'secret-b', SMUP_COOKIE: 'secret-c',
    SMOLUP_COOKIE_FILE: '/private/cookie', SMOP_TOKEN: 'secret-token', smup_cookie: 'secret-lowercase',
  };
  const cleaned = mediaProbeEnvironment(environment);
  assert.equal(cleaned.PATH, environment.PATH);
  assert.equal(cleaned.HOME, environment.HOME);
  for (const key of Object.keys(environment).filter(key => /^(?:SMOLUP|SMOP|SMUP)_/i.test(key))) assert.equal(cleaned[key], undefined);
  assert.equal(environment.SMOLUP_COOKIE, 'secret-a');
});

test('installer runner handles success, failure, timeout and cancellation', async () => {
  assert.match(await runSetup(process.execPath, ['-e', 'console.log("setup ready")']), /setup ready/);
  await assert.rejects(runSetup(process.execPath, ['-e', 'process.exit(7)']), /code 7/);
  await assert.rejects(runSetup(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { timeoutMs: 100 }), /timed out/);
  const controller = new AbortController();
  const pending = runSetup(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { signal: controller.signal });
  controller.abort(new Error('setup cancelled'));
  await assert.rejects(pending, /setup cancelled/);
});
