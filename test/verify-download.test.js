import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { parseCli } from '../src/cli.js';
import { download } from '../src/downloader.js';
import { verifySavedMedia } from '../src/inspect-media.js';

const url = 'https://example.test/video';
const backendResolver = async () => ({ ytDlp: 'fake', ffmpegLocation: 'tools' });

function runner() {
  return async (_, args, { onLine } = {}) => {
    if (args.includes('--dump-single-json')) return JSON.stringify({ id: 'video', title: 'Video', formats: [{ height: 720, vcodec: 'h264' }] });
    const file = path.join(path.dirname(args[args.indexOf('-o') + 1]), 'media.mp4');
    await writeFile(file, 'media');
    onLine?.(`veo-file:${JSON.stringify(file)}`);
    return '';
  };
}

test('verification is opt-in and can be set in a profile', () => {
  assert.equal(parseCli([url]).verify, false);
  assert.equal(parseCli([url, '--verify']).verify, true);
  assert.equal(parseCli([url], { config: { profiles: { checked: { verify: true } }, activeProfile: 'checked' } }).verify, true);
  assert.equal(parseCli([url, '--no-verify'], { config: { verify: true } }).verify, false);
});

test('optional verification probes saved media before recording output history', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-verify-'));
  let checked = 0;
  try {
    const base = { url, output: root };
    const plain = await download(base, { localRoot: path.join(root, 'cache'), backendResolver, runner: runner(),
      verifier: async () => { checked++; throw new Error('should not be called'); } });
    assert.equal(plain.verified, undefined);
    assert.equal(checked, 0);
    const verified = await download({ ...base, verify: true }, { localRoot: path.join(root, 'cache'), backendResolver, runner: runner(),
      verifier: async (file, settings) => {
        checked++;
        assert.equal(settings.audio, false);
        assert.equal(await readFile(file, 'utf8'), 'media');
      } });
    assert.equal(verified.verified, true);
    assert.equal(checked, 1);
    const skipped = await download({ ...base, verify: true, skipExisting: true }, { localRoot: path.join(root, 'cache'), backendResolver, runner: runner(),
      verifier: async () => { checked++; } });
    assert.equal(skipped.status, 'skipped');
    assert.equal(skipped.verified, true);
    assert.equal(checked, 2);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('failed verification retains saved file but no success record or reusable staging', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-verify-fail-'));
  try {
    let failure;
    try {
      await download({ url, output: root, verify: true }, { localRoot: path.join(root, 'cache'), backendResolver, runner: runner(),
        verifier: async () => { throw new Error('bad container'); } });
    } catch (error) { failure = error; }
    assert.match(failure.message, /bad container/);
    assert.equal(failure.verificationFailed, true);
    assert.equal(failure.files.length, 1);
    assert.equal(await readFile(failure.files[0], 'utf8'), 'media');
    assert.deepEqual((await readdir(root)).filter(name => name !== 'cache'), ['Video.mp4']);
    assert.deepEqual(await readdir(path.join(root, 'cache')), []);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('verification requires the expected stream and surfaces probe errors', async () => {
  await assert.rejects(verifySavedMedia('movie.mp4', { inspect: async () => ({ hasAudioTrack: true, hasVideoTrack: false }) }), /no video track/);
  await assert.rejects(verifySavedMedia('audio.mp3', { audio: true, inspect: async () => ({ hasAudioTrack: false, hasVideoTrack: false }) }), /no audio track/);
  await assert.rejects(verifySavedMedia('bad.mp4', { inspect: async () => { throw new Error('invalid container'); } }), /invalid container/);
  assert.equal((await verifySavedMedia('okay.mp4', { inspect: async () => ({ hasVideoTrack: true }) })).hasVideoTrack, true);
});
