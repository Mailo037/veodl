import test from 'node:test';
import assert from 'node:assert/strict';
import { parseCli } from '../src/cli.js';
import { listQualities } from '../src/downloader.js';
const url = 'https://example.test/video';
test('quality listing flags and incompatible actions', () => {
  for (const flag of ['--list-qualities', '--list-qualitys']) {
    assert.equal(parseCli([url, flag]).listQualities, true);
    for (const extra of [[url], ['--audio'], ['--playlist'], ['--list-formats'], ['--list-sources'], ['--batch-file', 'urls.txt'], ['--retry-failed', 'abc']]) {
      assert.throws(() => parseCli([url, flag, ...extra]));
    }
    assert.equal(parseCli([url, flag, '--source', '1', '--json']).source, '1');
  }
});
test('quality listing deduplicates real video resolutions without downloading', async () => {
  const calls = [];
  const dependencies = {
    backendResolver: async () => ({ ytDlp: 'fake', ffmpegLocation: 'tools' }),
    runner: async (_, args) => {
      calls.push(args);
      return JSON.stringify({ title: 'Example', formats: [
        { height: 720, vcodec: 'h264' }, { height: 1080, vcodec: 'vp9' },
        { height: 720, vcodec: 'vp9' }, { height: 2160, vcodec: 'none' },
        { height: 1440, vcodec: 'h264', has_drm: true },
        { height: 90, ext: 'mhtml' }, { vcodec: 'h264' },
      ] });
    },
  };
  const result = await listQualities({ url, mediaUrl: 'https://example.test/media', cookiesFromBrowser: 'chrome' }, dependencies);
  assert.deepEqual(result, { url, title: 'Example', status: 'qualities', qualities: ['1080p', '720p'] });
  assert.ok(calls[0].includes('--skip-download'));
  assert.ok(calls[0].includes('--cookies-from-browser'));
  assert.equal(calls[0].at(-1), 'https://example.test/media');
  await assert.rejects(listQualities({ url }, { ...dependencies, runner: async () => { throw new Error('backend failed'); } }), /backend failed/);
});
test('unknown resolution stays unknown and audio-only sources fail', async () => {
  const backendResolver = async () => ({ ytDlp: 'fake' });
  const runner = formats => async () => JSON.stringify({ formats });
  assert.deepEqual((await listQualities({ url }, { backendResolver, runner: runner([{ vcodec: 'h264' }]) })).qualities, []);
  await assert.rejects(listQualities({ url }, { backendResolver, runner: runner([{ vcodec: 'none' }]) }), /No downloadable video/);
});
