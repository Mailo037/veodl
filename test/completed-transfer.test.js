import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { resolvePageSource } from '../src/cli.js';
import { download, localRequestKey, partialKey, readReadyTransfer } from '../src/downloader.js';

async function fixture(t, extra = {}) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-completed-transfer-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const localRoot = path.join(root, 'cache');
  const options = { url: 'https://offline.example.test/video', output: path.join(root, 'output'), quality: 'best', ...extra };
  const requestKey = localRequestKey(options);
  const staging = path.join(localRoot, `.veo-part-${requestKey}`);
  await mkdir(staging, { recursive: true });
  const file = path.join(staging, 'media.mp4');
  await writeFile(file, 'completed media');
  const metadata = { id: 'video', title: 'Completed video', extractor_key: 'Generic' };
  const manifest = { version: 1, requestKey, key: partialKey(metadata, options.url, options), metadata,
    ready: [file], files: [], backendSucceeded: true, compatibilityChecked: true, expiresAt: Date.now() + 60_000 };
  const manifestFile = path.join(staging, 'job.json');
  await writeFile(manifestFile, JSON.stringify(manifest));
  return { root, localRoot, options, staging, file, manifest, manifestFile };
}

test('completed transfer preflight stays offline for an ordinary request and an explicit selected-source retry', async t => {
  for (const extra of [{}, { source: '1', resume: true }]) {
    const saved = await fixture(t, extra);
    const dependencies = { localRoot: saved.localRoot, stderr: { write() {} }, interactive: false,
      backendResolver: async () => assert.fail('completed transfer must not prepare a backend'),
      inspect: async () => assert.fail('completed transfer must not inspect its source'),
      discover: async () => assert.fail('completed transfer must not rediscover signed source URLs'),
    };
    assert.deepEqual(await resolvePageSource(saved.options, dependencies), {});
    const result = await download(saved.options, { localRoot: saved.localRoot,
      backendResolver: dependencies.backendResolver, runner: dependencies.inspect, checkSpace: false });
    assert.equal(result.status, 'saved');
    assert.equal(await readFile(result.files[0], 'utf8'), 'completed media');
  }
});

test('source listings, metadata actions, dry runs and incognito never skip validation because of cached transfer data', async t => {
  const saved = await fixture(t);
  for (const action of ['listSources', 'listFormats', 'listQualities', 'listSubs', 'dryRun', 'incognito']) {
    let prepared = 0, inspected = 0, discovered = 0;
    const result = await resolvePageSource({ ...saved.options, [action]: true }, {
      localRoot: saved.localRoot, stderr: { write() {} }, interactive: false,
      inspect: async () => { inspected++; return { id: 'video', formats: [{ height: 720 }] }; },
      discover: async () => { discovered++; return []; },
      onBackend() { prepared++; },
    });
    assert.equal(inspected + discovered, 1, `${action} must validate the source`);
    assert.equal(prepared, 0);
    if (action === 'listSources') assert.deepEqual(result.sources, []);
    else assert.deepEqual(result, {});
  }
});

test('completed readiness rejects expired, unconfirmed, mismatched and invalid-path records', async t => {
  const saved = await fixture(t);
  const corruptions = [
    { expiresAt: 0 }, { expiresAt: 'later' }, { version: 2 }, { requestKey: 'unrelated' }, { key: 'unrelated' },
    { backendSucceeded: false, compatibilityChecked: false }, { metadata: [] }, { ready: [] },
    { ready: [path.join(saved.root, 'outside.mp4')] }, { ready: [path.join(saved.staging, 'media.mp4.part')] },
    { files: [{ source: saved.file, destination: path.join(saved.root, 'outside.mp4') }] },
  ];
  for (const changes of corruptions) {
    await writeFile(saved.manifestFile, JSON.stringify({ ...saved.manifest, ...changes }));
    assert.equal(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), null, JSON.stringify(changes));
    let inspected = 0;
    await resolvePageSource(saved.options, { localRoot: saved.localRoot, stderr: { write() {} },
      inspect: async () => { inspected++; return { formats: [{ height: 720 }] }; } });
    assert.equal(inspected, 1, 'unusable completed state must not suppress source validation');
  }
  await writeFile(saved.manifestFile, JSON.stringify(saved.manifest));
  assert.deepEqual(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), saved.manifest);
  await rm(saved.file);
  assert.equal(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), null);
  await writeFile(saved.file, 'completed media');
  await writeFile(saved.manifestFile, '{malformed');
  assert.equal(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), null);
});

test('completed cache does not accept symlink media files or a symlink staging directory', async t => {
  const saved = await fixture(t);
  const outside = path.join(saved.root, 'external.mp4');
  await writeFile(outside, 'external media');
  await rm(saved.file);
  try { await symlink(outside, saved.file); }
  catch (error) {
    if (['EPERM', 'EACCES', 'ENOTSUP'].includes(error.code)) { t.skip('File symlinks are unavailable on this host.'); return; }
    throw error;
  }
  assert.equal(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), null);
  await rm(saved.staging, { recursive: true });
  const externalDirectory = path.join(saved.root, 'external-directory');
  await mkdir(externalDirectory);
  await symlink(externalDirectory, saved.staging, process.platform === 'win32' ? 'junction' : 'dir');
  assert.equal(await readReadyTransfer(saved.options, { localRoot: saved.localRoot }), null);
});

test('completed transfer lookup respects cancellation and does not modify cache data', async t => {
  const saved = await fixture(t);
  const before = await readFile(saved.manifestFile, 'utf8');
  await readReadyTransfer(saved.options, { localRoot: saved.localRoot });
  assert.equal(await readFile(saved.manifestFile, 'utf8'), before);
  const controller = new AbortController();
  controller.abort(new Error('cancelled'));
  await assert.rejects(readReadyTransfer(saved.options, { localRoot: saved.localRoot, signal: controller.signal }), /cancelled/);
});
