import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { exeSuffix, resolveMediaTools } from '../src/backend.js';
import { ensureCompatibility } from '../src/compatibility.js';
import { runBackend } from '../src/downloader.js';
import { parseCli } from '../src/cli.js';

test('compatibility CLI is explicit and rejects conflicting media settings', () => {
  const options = parseCli(['https://example.test/video', '--compatible']);
  assert.equal(options.format, 'mp4');
  assert.equal(options.compatible, true);
  for (const args of [['--audio'], ['--format', 'webm'], ['--embed-subs'], ['--recode', '--format', 'mp4']]) assert.throws(() => parseCli(['https://example.test/video', '--compatible', ...args]), /requires video MP4/);
});

test('compatibility conversion really produces H264 AAC and copies already compatible MP4', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-compat-'));
  const file = path.join(directory, 'media.mp4');
  try {
    const ffmpegLocation = await resolveMediaTools({ offline: true });
    const backend = { ffmpegLocation };
    const ffmpeg = path.join(ffmpegLocation, `ffmpeg${exeSuffix()}`);
    const probe = path.join(ffmpegLocation, `ffprobe${exeSuffix()}`);
    const runner = (command, args, options) => runBackend(command, args, options);
    await runBackend(ffmpeg, ['-v', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=160x90:r=24', '-f', 'lavfi', '-i', 'sine=frequency=440', '-t', '0.3', '-c:v', 'mpeg4', '-c:a', 'aac', file]);
    const original = await readFile(file), messages = [];
    await ensureCompatibility(file, { backend, runner, reporter: { status: message => messages.push(message) } });
    assert.deepEqual(await readFile(file), original);
    assert.match(messages[0], /Original quality retained/);
    await ensureCompatibility(file, { backend, runner, convert: true });
    const streams = JSON.parse(await runBackend(probe, ['-v', 'error', '-show_streams', '-of', 'json', file])).streams;
    assert.equal(streams.find(stream => stream.codec_type === 'video').codec_name, 'h264');
    assert.equal(streams.find(stream => stream.codec_type === 'video').pix_fmt, 'yuv420p');
    assert.equal(streams.find(stream => stream.codec_type === 'audio').codec_name, 'aac');
    const compatible = await readFile(file);
    await ensureCompatibility(file, { backend, runner, convert: true });
    assert.deepEqual(await readFile(file), compatible);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
