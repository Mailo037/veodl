import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile, chmod } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { selectAsset, staticToolsSupported, findOnPath, wellKnownMediaDirectories, backendCacheDirectory, exeSuffix, inspectBackend, resolveBackend, resolveMediaTools, RELEASE } from '../src/backend.js';

test('Android resolves system tools offline without acquiring desktop binaries', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-android-'));
  const saved = [process.env.VEO_YT_DLP_PATH, process.env.VEO_FFMPEG_PATH];
  delete process.env.VEO_YT_DLP_PATH;
  delete process.env.VEO_FFMPEG_PATH;
  try {
    for (const name of ['yt-dlp', 'ffmpeg', 'ffprobe']) {
      await writeFile(path.join(dir, name), '#!/bin/sh\n');
      await chmod(path.join(dir, name), 0o755);
    }
    const find = names => findOnPath(names, { platform: 'android', directories: [dir] });
    const report = await inspectBackend({ platform: 'android', arch: 'arm64', find });
    assert.equal(report.ytDlp.source, 'system');
    assert.equal(report.ytDlp.verified, false);
    assert.deepEqual(report.errors, []);
    assert.deepEqual(await resolveBackend({ platform: 'android', offline: true, find, hasEjs: async () => true }), {
      ytDlp: path.join(dir, 'yt-dlp'), ffmpegLocation: dir,
    });
    await assert.rejects(resolveBackend({ platform: 'android', offline: true, find: async () => undefined }), /pkg install python-yt-dlp/);
    await assert.rejects(resolveBackend({ platform: 'android', offline: true, find: async names => names[0] === 'yt-dlp' ? path.join(dir, 'yt-dlp') : undefined }), /without --offline/);
    process.env.VEO_YT_DLP_PATH = path.join(dir, 'missing');
    await assert.rejects(resolveBackend({ platform: 'android', find }), /VEO_YT_DLP_PATH is missing/);
    process.env.VEO_YT_DLP_PATH = path.join(dir, 'yt-dlp');
    process.env.VEO_FFMPEG_PATH = dir;
    assert.equal((await resolveBackend({ platform: 'android', find: async () => { throw new Error('unexpected lookup'); } })).ffmpegLocation, dir);
  } finally {
    for (const [index, key] of ['VEO_YT_DLP_PATH', 'VEO_FFMPEG_PATH'].entries()) {
      if (saved[index] === undefined) delete process.env[key]; else process.env[key] = saved[index];
    }
    await rm(dir, { recursive: true, force: true });
  }
});

test('Android does not select Linux assets and discovers Termux prefix tools', () => {
  assert.equal(selectAsset('android', 'arm64'), undefined);
  assert.equal(staticToolsSupported('android', 'arm64'), false);
  assert.deepEqual(wellKnownMediaDirectories({ platform: 'android', env: { PREFIX: '/data/data/com.termux/files/usr' } }), [path.join('/data/data/com.termux/files/usr', 'bin')]);
});

test('standalone asset selection covers every published combination', () => {
  assert.equal(selectAsset('win32', 'x64'), 'yt-dlp.exe');
  assert.equal(selectAsset('win32', 'arm64'), 'yt-dlp_arm64.exe');
  assert.equal(selectAsset('win32', 'ia32'), 'yt-dlp_x86.exe');
  assert.equal(selectAsset('darwin', 'arm64'), 'yt-dlp_macos');
  assert.equal(selectAsset('linux', 'x64'), 'yt-dlp_linux');
  assert.equal(selectAsset('linux', 'x64', true), 'yt-dlp_musllinux');
  assert.equal(selectAsset('linux', 'arm64'), 'yt-dlp_linux_aarch64');
  assert.equal(selectAsset('linux', 'arm64', true), 'yt-dlp_musllinux_aarch64');
  // Unsupported combinations must stay undefined instead of guessing.
  for (const [platform, arch] of [['darwin', 'ia32'], ['linux', 'ia32'], ['freebsd', 'x64'], ['win32', 'mips']]) {
    assert.equal(selectAsset(platform, arch), undefined, `${platform}/${arch}`);
  }
});

test('desktop media-tool platform discovery keeps its existing matrix', () => {
  for (const [platform, arch, expected] of [
    ['win32', 'x64', true],
    // Windows on ARM uses the verified x64 pair with OS emulation.
    ['win32', 'arm64', true],
    ['linux', 'arm64', true],
    ['darwin', 'arm64', true],
    ['darwin', 'ia32', false],
    ['linux', 'riscv64', false],
    ['win32', 'mips', false],
    ['freebsd', 'x64', false],
  ]) assert.equal(staticToolsSupported(platform, arch), expected, `${platform}/${arch}`);
});

test('executable legacy media caches cannot bypass verification, including offline', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-unverified-media-'));
  try {
    for (const name of ['ffmpeg', 'ffprobe']) {
      const file = path.join(directory, `${name}${exeSuffix()}`);
      await writeFile(file, 'unverified binary');
      await chmod(file, 0o755);
    }
    let installs = 0;
    const options = { directory, find: async () => undefined,
      install: async () => { installs++; return directory; } };
    await assert.rejects(resolveMediaTools({ ...options, offline: true }), /SHA-256 verification/);
    assert.equal(installs, 0);
    assert.equal(await resolveMediaTools(options), directory);
    assert.equal(installs, 1);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('PATH discovery finds executables and ignores non-absolute entries', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-path-'));
  try {
    const fake = path.join(dir, `veo-ffmpeg-probe${exeSuffix()}`);
    await writeFile(fake, '#!/bin/sh\n');
    await chmod(fake, 0o755);
    const env = { PATH: ['relative/dir', dir, path.join(dir, 'missing')].join(path.delimiter) };
    assert.equal(await findOnPath(['veo-ffmpeg-probe'], { platform: process.platform, env }), fake);
    assert.equal(await findOnPath(['veo-ffmpeg-probe'], { platform: process.platform, env, directories: [dir] }), fake);
    assert.equal(await findOnPath(['veo-ffmpeg-probe'], { platform: process.platform, env: { PATH: path.join(dir, 'missing') }, directories: [] }), undefined);
    assert.equal(await findOnPath(['veo-absent-tool'], { platform: process.platform, env, directories: [dir] }), undefined);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('well-known media directories are absolute and platform-appropriate', () => {
  // Windows-style candidates must be judged with the Windows path rules, not
  // with those of whatever host happens to run the tests.
  for (const directory of wellKnownMediaDirectories({ platform: 'win32', env: { LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local', ProgramFiles: 'C:\\Program Files' } })) {
    assert.ok(path.win32.isAbsolute(directory), directory);
  }
  // Without LOCALAPPDATA or ProgramFiles the list must still hold no broken entries.
  for (const directory of wellKnownMediaDirectories({ platform: 'win32', env: {} })) {
    assert.ok(path.win32.isAbsolute(directory), directory);
  }
  for (const directory of wellKnownMediaDirectories({ platform: 'linux', env: {} })) {
    assert.ok(path.posix.isAbsolute(directory), directory);
  }
  assert.ok(wellKnownMediaDirectories({ platform: 'linux', env: {} }).includes('/usr/bin'));
});

test('the backend cache is versioned and platform-specific', () => {
  const directory = backendCacheDirectory(RELEASE);
  assert.ok(directory.includes(path.join('veo', 'backends', RELEASE)));
  assert.ok(directory.endsWith(`${process.platform}-${process.arch}`));
  assert.notEqual(backendCacheDirectory('2099.01.01'), directory);
});

test('inspection reports state without downloading or executing anything', async () => {
  const report = await inspectBackend();
  assert.equal(report.release, RELEASE);
  assert.equal(report.directory, backendCacheDirectory(RELEASE));
  assert.ok(['managed', 'override', 'none', 'invalid'].includes(report.ytDlp.source));
  assert.equal(typeof report.ytDlp.present, 'boolean');
  for (const name of ['ffmpeg', 'ffprobe']) {
    assert.ok(['cache', 'override', 'path', 'missing'].includes(report[name].source));
    if (report[name].path) assert.ok(path.isAbsolute(report[name].path));
  }
});
