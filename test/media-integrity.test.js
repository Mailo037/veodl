import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile, readdir, rm, symlink } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { gzipSync } from 'node:zlib';
import { downloadMediaAsset, verifiedMediaTools } from '../src/tool-setup.js';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const payload = Buffer.from('small verified media fixture\n');
const zipped = gzipSync(payload);
const rawAsset = { name: 'ffmpeg-linux-x64', size: payload.length, sha256: digest(payload) };
const gzipAsset = { ...rawAsset, gzip: { name: `${rawAsset.name}.gz`, size: zipped.length, sha256: digest(zipped) } };
const release = 'b6.1.1';
const origin = `https://github.com/eugeneware/ffmpeg-static/releases/download/${release}`;

async function workspace(t) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-media-integrity-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return { directory, destination: path.join(directory, 'ffmpeg') };
}

test('raw and gzip media assets are written only after every pinned hash and size matches', async t => {
  for (const [asset, body] of [[rawAsset, payload], [gzipAsset, zipped]]) {
    const { directory, destination } = await workspace(t);
    const requests = [];
    await downloadMediaAsset({ asset, release, destination,
      fetchImpl: async (url, options) => {
        requests.push([url, options]);
        return new Response(body, { headers: { 'Content-Length': String(body.length) } });
      },
    });
    assert.deepEqual(await readFile(destination), payload);
    assert.deepEqual(await readdir(directory), ['ffmpeg']);
    assert.equal(requests.length, 1);
    assert.equal(requests[0][0], `${origin}/${asset.gzip?.name || asset.name}`);
    assert.equal(requests[0][1].redirect, 'manual');
    assert.ok(requests[0][1].signal instanceof AbortSignal);
    assert.equal(requests[0][1].headers?.Cookie, undefined);
    assert.equal(requests[0][1].headers?.Authorization, undefined);
  }
});

test('a modified compressed asset is discarded before gzip expansion', async t => {
  const { directory, destination } = await workspace(t);
  const modified = Buffer.from(zipped);
  modified[modified.length - 1] ^= 1;
  await assert.rejects(downloadMediaAsset({ asset: gzipAsset, release, destination,
    fetchImpl: async () => new Response(modified),
  }), /SHA-256|checksum/i);
  assert.deepEqual(await readdir(directory), []);
});

test('matching compressed bytes cannot bypass the decompressed SHA-256 pin', async t => {
  const { directory, destination } = await workspace(t);
  await assert.rejects(downloadMediaAsset({ asset: { ...gzipAsset, sha256: '0'.repeat(64) }, release, destination,
    fetchImpl: async () => new Response(zipped),
  }), /SHA-256|checksum/i);
  assert.deepEqual(await readdir(directory), []);
});

test('a wrong raw hash leaves no acquired file', async t => {
  const { directory, destination } = await workspace(t);
  await assert.rejects(downloadMediaAsset({ asset: { ...rawAsset, sha256: '0'.repeat(64) }, release, destination,
    fetchImpl: async () => new Response(payload),
  }), /SHA-256|checksum/i);
  assert.deepEqual(await readdir(directory), []);
});

test('an existing destination is preserved when acquisition fails', async t => {
  const { directory, destination } = await workspace(t);
  await writeFile(destination, 'existing trusted file');
  await assert.rejects(downloadMediaAsset({ asset: { ...rawAsset, sha256: '0'.repeat(64) }, release, destination,
    fetchImpl: async () => new Response(payload),
  }));
  assert.equal(await readFile(destination, 'utf8'), 'existing trusted file');
  assert.deepEqual(await readdir(directory), ['ffmpeg']);
});

test('oversized Content-Length is rejected before streaming and cancels the response', async t => {
  const { directory, destination } = await workspace(t);
  let cancelled = false;
  const body = new ReadableStream({ cancel() { cancelled = true; } });
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination,
    fetchImpl: async () => new Response(body, { headers: { 'Content-Length': String(payload.length + 1) } }),
  }), /size|limit|length/i);
  assert.equal(cancelled, true);
  assert.deepEqual(await readdir(directory), []);
});

test('streamed bytes are bounded even when Content-Length is absent', async t => {
  const { directory, destination } = await workspace(t);
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination,
    fetchImpl: async () => new Response(Buffer.concat([payload, Buffer.from('overflow')])),
  }), /size|limit|length/i);
  assert.deepEqual(await readdir(directory), []);
});

test('gzip expansion is bounded by the pinned decompressed size', async t => {
  const { directory, destination } = await workspace(t);
  const expansion = gzipSync(Buffer.alloc(16_384, 65));
  const asset = { ...rawAsset, gzip: { name: `${rawAsset.name}.gz`, size: expansion.length, sha256: digest(expansion) } };
  await assert.rejects(downloadMediaAsset({ asset, release, destination,
    fetchImpl: async () => new Response(expansion),
  }), /size|limit|length/i);
  assert.deepEqual(await readdir(directory), []);
});

test('truncated bytes are discarded even if their hash was separately supplied', async t => {
  const { directory, destination } = await workspace(t);
  const truncated = payload.subarray(0, payload.length - 1);
  await assert.rejects(downloadMediaAsset({ asset: { ...rawAsset, sha256: digest(truncated) }, release, destination,
    fetchImpl: async () => new Response(truncated),
  }), /size|limit|length/i);
  assert.deepEqual(await readdir(directory), []);
});

test('HTTP failures cancel the response and leave no downloaded files', async t => {
  const { directory, destination } = await workspace(t);
  let cancelled = false;
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination,
    fetchImpl: async () => new Response(new ReadableStream({ cancel() { cancelled = true; } }), { status: 503 }),
  }), /503/);
  assert.equal(cancelled, true);
  assert.deepEqual(await readdir(directory), []);
});

test('official GitHub asset redirects preserve verification', async t => {
  const { destination } = await workspace(t);
  const redirected = 'https://release-assets.githubusercontent.com/github-production-release-asset/fixture';
  const requests = [];
  await downloadMediaAsset({ asset: rawAsset, release, destination,
    fetchImpl: async url => {
      requests.push(url);
      return requests.length === 1
        ? new Response(null, { status: 302, headers: { Location: redirected } })
        : new Response(payload);
    },
  });
  assert.deepEqual(requests, [`${origin}/${rawAsset.name}`, redirected]);
  assert.deepEqual(await readFile(destination), payload);
});

test('redirects cannot send asset requests to another host or insecure URL', async t => {
  for (const location of [
    'https://attacker.invalid/media',
    'https://github.com.attacker.invalid/media',
    'http://github.com/media',
    'https://token@release-assets.githubusercontent.com/media',
  ]) {
    const { directory, destination } = await workspace(t);
    let requests = 0;
    await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination,
      fetchImpl: async () => {
        requests++;
        return new Response(null, { status: 302, headers: { Location: location } });
      },
    }), /redirect|URL|host|https|credential/i);
    assert.equal(requests, 1);
    assert.deepEqual(await readdir(directory), []);
  }
});

test('redirect loops are bounded', async t => {
  const { directory, destination } = await workspace(t);
  let requests = 0;
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination,
    fetchImpl: async url => {
      requests++;
      return new Response(null, { status: 302, headers: { Location: url } });
    },
  }), /redirect/i);
  assert.ok(requests <= 10);
  assert.deepEqual(await readdir(directory), []);
});

test('unsafe asset metadata is rejected before any HTTP request', async t => {
  const { directory, destination } = await workspace(t);
  let requests = 0;
  const fetchImpl = async () => { requests++; return new Response(payload); };
  for (const asset of [
    { ...rawAsset, name: '../ffmpeg' }, { ...rawAsset, name: 'ffmpeg/other' },
    { ...rawAsset, name: 'https://attacker.invalid/ffmpeg' },
    { ...rawAsset, sha256: '' }, { ...rawAsset, size: -1 }, { ...rawAsset, size: 1.5 },
    { ...gzipAsset, gzip: { ...gzipAsset.gzip, name: '../ffmpeg.gz' } },
  ]) await assert.rejects(downloadMediaAsset({ asset, release, destination, fetchImpl }));
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release: '../latest', destination, fetchImpl }));
  assert.equal(requests, 0);
  assert.deepEqual(await readdir(directory), []);
});

test('an aborted signal prevents downloads', async t => {
  const { directory, destination } = await workspace(t);
  const controller = new AbortController();
  controller.abort(new Error('fixture cancelled'));
  let requests = 0;
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination, signal: controller.signal,
    fetchImpl: async () => { requests++; return new Response(payload); },
  }), /fixture cancelled/);
  assert.equal(requests, 0);
  assert.deepEqual(await readdir(directory), []);
});

test('cancellation during streaming discards partial files', async t => {
  const { directory, destination } = await workspace(t);
  const controller = new AbortController();
  const body = new ReadableStream({ start(stream) { stream.enqueue(payload.subarray(0, 3)); } });
  const pending = downloadMediaAsset({ asset: rawAsset, release, destination, signal: controller.signal,
    fetchImpl: async () => new Response(body),
  });
  const cancelled = assert.rejects(pending, /fixture stream cancelled|abort/i);
  try {
    let partial;
    for (let attempts = 0; attempts < 100; attempts++) {
      partial = await readFile(destination).catch(error => {
        if (error.code === 'ENOENT') return undefined;
        throw error;
      });
      if (partial?.length) break;
      await new Promise(resolve => setTimeout(resolve, 5));
    }
    assert.equal(partial?.length, 3);
  } finally {
    controller.abort(new Error('fixture stream cancelled'));
    await cancelled;
  }
  assert.deepEqual(await readdir(directory), []);
});

test('a stalled request times out and cleans temporary files', async t => {
  const { directory, destination } = await workspace(t);
  await assert.rejects(downloadMediaAsset({ asset: rawAsset, release, destination, timeoutMs: 30,
    fetchImpl: async (url, { signal }) => new Promise((resolve, reject) => {
      const stop = () => reject(signal.reason);
      signal.addEventListener('abort', stop, { once: true });
      if (signal.aborted) stop();
    }),
  }), /timeout|timed out/i);
  assert.deepEqual(await readdir(directory), []);
});

test('media cache verification rejects absent, incomplete and tampered binary pairs', async t => {
  const { directory } = await workspace(t);
  const options = { directory, platform: 'linux', arch: 'x64' };
  assert.equal(await verifiedMediaTools(options), false);
  await writeFile(path.join(directory, 'ffmpeg'), payload);
  assert.equal(await verifiedMediaTools(options), false);
  await writeFile(path.join(directory, 'ffprobe'), payload);
  assert.equal(await verifiedMediaTools(options), false);
  assert.equal(await verifiedMediaTools({ ...options, platform: 'android', arch: 'arm64' }), false);
});

test('media cache verification rejects symlinked binaries', async t => {
  const { directory } = await workspace(t);
  const target = path.join(directory, 'external-file');
  await writeFile(target, payload);
  try { await symlink(target, path.join(directory, 'ffmpeg'), 'file'); }
  catch (error) {
    if (['EPERM', 'EACCES', 'ENOSYS'].includes(error.code)) { t.skip('File symlinks are unavailable on this host.'); return; }
    throw error;
  }
  await writeFile(path.join(directory, 'ffprobe'), payload);
  assert.equal(await verifiedMediaTools({ directory, platform: 'linux', arch: 'x64' }), false);
});
