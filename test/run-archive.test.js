import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rename, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { exeSuffix, resolveMediaTools } from '../src/backend.js';
import { inspectMain, inspectRun, runMediaTool } from '../src/inspect-media.js';
import { runJob } from '../src/jobs.js';
import { createHistoryRecorder, readHistory } from '../src/history.js';
import { archiveRun, readRunArchive } from '../src/run-archive.js';
import { flush } from '../src/flush.js';
import { cacheBase } from '../src/paths.js';

const sink = () => { let value = ''; return { stream: { write: chunk => { value += chunk; } }, read: () => value }; };

test('finished run keeps its id and locates renamed media before inspection', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-run-archive-'));
  const id = 'abc123';
  const original = path.join(root, 'Original.mp4');
  const renamed = path.join(root, 'Renamed.media');
  const stdout = sink(), stderr = sink();
  try {
    assert.equal(await runJob({ urls: ['https://example.test/media'], output: root, json: true }, {
      runId: id, archiveRoot: root, jobFile: path.join(root, 'jobs', 'job.json'),
      stdout: stdout.stream, stderr: stderr.stream, reporter: { complete() {}, fail() {} },
      recordHistory: createHistoryRecorder(root),
      download: async () => {
        await writeFile(original, Buffer.alloc(200_000, 42));
        return { url: 'https://example.test/media', title: 'Original', status: 'saved', files: [original] };
      },
    }), 0);
    assert.equal(JSON.parse(stdout.read()).runId, id);
    assert.equal((await readHistory(root))[0].runId, id);
    assert.equal((await readRunArchive(id, root)).items[0].files[0].originalPath, original);
    await flush({ root });
    assert.equal((await readRunArchive(id, root)).id, id, 'normal cleanup keeps finished run metadata');

    await rename(original, renamed);
    let probed = 0;
    const inspect = async file => {
      probed++;
      assert.equal(file, renamed);
      assert.ok((await stat(file)).isFile());
      return { file, format: 'mp4', streams: [{ type: 'audio' }], audioCheck: null };
    };
    const found = await inspectRun(id, { root, inspect });
    assert.equal(found.missingFiles, 0);
    assert.equal(found.items[0].files[0].renamed, true);
    assert.equal(found.items[0].files[0].metadata.format, 'mp4');
    assert.equal(probed, 1);

    await rm(renamed);
    const missing = await inspectRun(id, { root, inspect });
    assert.equal(missing.missingFiles, 1);
    assert.equal(missing.items[0].files[0].metadata, null);
    assert.equal(probed, 1, 'missing files must never be sent to ffprobe');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('inspect run accepts a search directory after a move and reports missing files', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-run-search-'));
  const output = path.join(root, 'output');
  const moved = path.join(root, 'moved');
  const original = path.join(output, 'Original.mp4');
  const relocated = path.join(moved, 'Changed.mp4');
  const { mkdir } = await import('node:fs/promises');
  try {
    await mkdir(output);
    await mkdir(moved);
    await writeFile(original, Buffer.alloc(200_000, 19));
    await archiveRun({ runId: 'def456', options: { output }, items: [{ url: 'https://example.test/v', status: 'saved', files: [original] }] }, { root });
    await rename(original, relocated);
    const noSearch = await inspectRun('def456', { root, inspect: async () => { throw new Error('should not probe'); } });
    assert.equal(noSearch.missingFiles, 1);
    const found = await inspectRun('def456', { root, searchRoot: moved,
      inspect: async file => ({ file, format: 'mp4', streams: [], audioCheck: null }) });
    assert.equal(found.items[0].files[0].path, relocated);
    assert.equal(found.missingFiles, 0);

    const json = sink();
    assert.equal(await inspectMain(['run', 'def456', '--search', moved, '--json'], { stdout: json.stream,
      inspectRunImpl: async (_, options) => { assert.equal(options.searchRoot, moved); return found; } }), 0);
    assert.equal(JSON.parse(json.read()).runId, 'def456');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('a file renamed before a long run ends keeps its early fingerprint', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-run-early-'));
  const first = path.join(root, 'First.mp4');
  const changed = path.join(root, 'First renamed.mp4');
  const second = path.join(root, 'Second.mp4');
  try {
    await runJob({ urls: ['https://example.test/list'], output: root }, {
      runId: 'jkl012', archiveRoot: root, jobFile: path.join(root, 'job.json'),
      stdout: sink().stream, stderr: sink().stream, reporter: { complete() {}, fail() {} },
      download: async (_, { onEntry }) => {
        await writeFile(first, Buffer.alloc(200_000, 7));
        await onEntry({ index: 1, status: 'saved', files: [first] });
        await rename(first, changed);
        await writeFile(second, Buffer.alloc(200_000, 8));
        await onEntry({ index: 2, status: 'saved', files: [second] });
        return { title: 'List', status: 'saved', files: [first, second] };
      },
    });
    const located = await inspectRun('jkl012', { root,
      inspect: async file => ({ file, format: 'mp4', streams: [], audioCheck: null }) });
    assert.equal(located.items[0].files[0].path, changed);
    assert.equal(located.items[0].files[1].path, second);
    assert.equal(located.missingFiles, 0);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('real CLI inspects a finished run after the media file was renamed', async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), 'veo-run-cli-'));
  const output = path.join(home, 'output');
  const original = path.join(output, 'Audio.wav');
  const renamed = path.join(output, 'Renamed.data');
  const env = { ...process.env, HOME: home, USERPROFILE: home,
    LOCALAPPDATA: home, XDG_CACHE_HOME: home, VEO_CONFIG: path.join(home, 'missing-config.json') };
  const root = cacheBase({ env, home });
  try {
    const { mkdir } = await import('node:fs/promises');
    await mkdir(output);
    const mediaDirectory = await resolveMediaTools({ offline: true });
    env.VEO_FFMPEG_PATH = mediaDirectory;
    await runMediaTool(path.join(mediaDirectory, `ffmpeg${exeSuffix()}`), ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
      '-i', 'sine=frequency=440:duration=0.2', '-c:a', 'pcm_s16le', original]);
    await archiveRun({ runId: 'ghi789', options: { output }, items: [{ url: 'https://example.test/audio', status: 'saved', files: [original] }] }, { root });
    await rename(original, renamed);
    const result = await runMediaTool(process.execPath, [path.resolve('bin/veo.js'), 'inspect', 'run', 'ghi789', '--check-audio', '--json'], { env });
    const parsed = JSON.parse(result.stdout);
    assert.equal(parsed.runId, 'ghi789');
    assert.equal(parsed.items[0].files[0].path, renamed);
    assert.equal(parsed.items[0].files[0].metadata.audioCheck.hasSignal, true);
  } finally { await rm(home, { recursive: true, force: true }); }
});
