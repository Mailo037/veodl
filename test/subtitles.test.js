import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { parseCli } from '../src/cli.js';
import { download, listSubtitles, partialKey } from '../src/downloader.js';

const url = 'https://example.test/video';
const backendResolver = async () => ({ ytDlp: 'fake', ffmpegLocation: 'tools' });

test('subtitle listing separates manual and automatic tracks without exposing URLs', async () => {
  const calls = [];
  const result = await listSubtitles({ url }, { backendResolver, runner: async (_, args) => {
    calls.push(args);
    return JSON.stringify({ title: 'Example', formats: [{ height: 720, vcodec: 'h264' }],
      subtitles: { de: [{ ext: 'vtt', name: 'German', url: 'secret-manual' }, { ext: 'srt', url: 'secret-manual-2' }] },
      automatic_captions: { en: [{ ext: 'vtt', name: 'English', url: 'secret-auto' }] } });
  } });
  assert.deepEqual(result.manual, [{ language: 'de', name: 'German', formats: ['srt', 'vtt'] }]);
  assert.deepEqual(result.automatic, [{ language: 'en', name: 'English', formats: ['vtt'] }]);
  assert.equal(result.status, 'subtitles');
  assert.doesNotMatch(JSON.stringify(result), /secret-/);
  assert.ok(calls[0].includes('--skip-download'));
  assert.deepEqual((await listSubtitles({ url }, { backendResolver, runner: async () => JSON.stringify({ title: 'No subs' }) })).manual, []);
});

test('subtitle flags select generated tracks, languages and source format', () => {
  const options = parseCli([url, '--auto-subs', '--sub-langs', 'de,en', '--sub-format', 'srt/vtt/best']);
  assert.equal(options.autoSubs, true);
  assert.equal(options.subs, false);
  assert.equal(options.subLangs, 'de,en');
  assert.equal(options.subFormat, 'srt/vtt/best');
  assert.equal(parseCli([url, '--subs', '--auto-subs']).subs, true);
  assert.equal(parseCli([url, '--list-subs']).listSubs, true);
  assert.throws(() => parseCli([url, '--list-subs', '--playlist']), /single video/);
  assert.throws(() => parseCli([url, '--sub-format', '../bad']), /sub-format/);
  const metadata = { id: 'video', extractor: 'Test' };
  assert.equal(partialKey(metadata, url), partialKey(metadata, url, { autoSubs: false }));
  assert.notEqual(partialKey(metadata, url), partialKey(metadata, url, { autoSubs: true }));
  assert.notEqual(partialKey(metadata, url), partialKey(metadata, url, { subFormat: 'srt' }));
});

test('automatic subtitle download forwards only requested track types and format', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-subs-'));
  const calls = [];
  const runner = async (_, args, { onLine } = {}) => {
    calls.push(args);
    if (args.includes('--dump-single-json')) return JSON.stringify({ id: 'video', title: 'Video', formats: [{ height: 720, vcodec: 'h264' }] });
    const file = path.join(path.dirname(args[args.indexOf('-o') + 1]), 'media.mp4');
    await writeFile(file, 'media');
    onLine?.(`veo-file:${JSON.stringify(file)}`);
    return '';
  };
  try {
    const options = parseCli([url, '--auto-subs', '--sub-langs', 'de', '--sub-format', 'srt/vtt', '--output', root]);
    const result = await download(options, { localRoot: path.join(root, 'cache'), backendResolver, runner });
    assert.equal(await readFile(result.files[0], 'utf8'), 'media');
    const mediaArgs = calls.find(args => args.includes('-o'));
    assert.ok(mediaArgs.includes('--write-auto-subs'));
    assert.ok(!mediaArgs.includes('--write-subs'));
    assert.equal(mediaArgs[mediaArgs.indexOf('--sub-langs') + 1], 'de');
    assert.equal(mediaArgs[mediaArgs.indexOf('--sub-format') + 1], 'srt/vtt');
  } finally { await rm(root, { recursive: true, force: true }); }
});
