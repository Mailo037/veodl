import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { HISTORY_HELP, HISTORY_LIMIT, createHistoryRecorder, historyMain, historyRoot, readHistory } from '../src/history.js';
import { runJob } from '../src/jobs.js';
import { flush } from '../src/flush.js';

const sink = () => { let text = ''; return { stdout: { write: chunk => { text += chunk; } }, read: () => text }; };
const escape = value => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

test('history bounds terminal titles and details while JSON preserves original values', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-width-'));
  try {
    const long = 'https://example.test/' + 'token'.repeat(100);
    await createHistoryRecorder(root)({ url: long, title: long, status: 'failed', files: [], error: long });
    const display = sink();
    Object.assign(display.stdout, { isTTY: true, columns: 50 });
    await historyMain(['--no-color'], { root, stdout: display.stdout });
    const lines = display.read().trimEnd().split('\n');
    assert.ok(lines[4].endsWith('…'));
    for (const line of lines) assert.ok(line.length < 50, line);
    const json = sink();
    Object.assign(json.stdout, { isTTY: true, columns: 50 });
    await historyMain(['--json'], { root, stdout: json.stdout });
    assert.equal(JSON.parse(json.read()).entries[0].title, long);
    assert.equal(JSON.parse(json.read()).entries[0].url, long);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('veo history shows only the newest five downloads, newest first', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-'));
  try {
    let clock = 1700000000000;
    const record = createHistoryRecorder(root, { now: () => (clock += 1000) });
    const file = index => `C:/Videos/Video ${index}.mp4`;
    for (let index = 1; index <= 7; index++) {
      await record({ url: `https://example.test/v${index}`, title: `Video ${index}`, status: index === 2 ? 'failed' : 'saved', quality: '1080p', files: [file(index)], elapsedMs: 1500 });
    }
    assert.equal((await readHistory(root, 100)).length, 7);

    const json = sink();
    assert.equal(await historyMain(['--json'], { root, stdout: json.stdout }), 0);
    const parsed = JSON.parse(json.read());
    assert.equal(parsed.count, HISTORY_LIMIT);
    assert.deepEqual(parsed.entries.map(entry => entry.title), ['Video 7', 'Video 6', 'Video 5', 'Video 4', 'Video 3']);
    assert.deepEqual(parsed.entries[0], {
      version: 1, at: parsed.entries[0].at, url: 'https://example.test/v7', title: 'Video 7',
      status: 'saved', media: 'video', quality: '1080p', format: null, files: [file(7)], error: null, elapsedMs: 1500,
    });
    assert.match(parsed.entries[0].at, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);

    const text = sink();
    assert.equal(await historyMain([], { root, stdout: text.stdout }), 0);
    assert.match(text.read(), /^veo history \(last 5, newest first\)/);
    assert.match(text.read(), /1\. Video 7/);
    assert.match(text.read(), /Status: saved/);
    assert.match(text.read(), /Media:  video, 1080p/);
    assert.match(text.read(), /URL:    https:\/\/example\.test\/v7/);
    assert.match(text.read(), new RegExp(`Saved:  ${escape(file(7))}`));
    assert.match(text.read(), /5\. Video 3/);
    assert.ok(!text.read().includes('Video 2'), 'older downloads must not be listed');

    const longer = sink();
    await historyMain(['--limit', '7'], { root, stdout: longer.stdout });
    assert.match(longer.read(), /7\. Video 1/);
    const failed = sink();
    await historyMain(['--json', '--failed', '--limit', '1'], { root, stdout: failed.stdout });
    assert.deepEqual(JSON.parse(failed.read()).entries.map(entry => entry.title), ['Video 2']);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('veo history help, empty history and usage errors', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-empty-'));
  try {
    const help = sink();
    assert.equal(await historyMain(['--help'], { root, stdout: help.stdout }), 0);
    assert.equal(help.read(), HISTORY_HELP);

    const json = sink();
    assert.equal(await historyMain(['--json'], { root, stdout: json.stdout }), 0);
    assert.deepEqual(JSON.parse(json.read()), { count: 0, entries: [] });

    const text = sink();
    assert.equal(await historyMain([], { root, stdout: text.stdout }), 0);
    assert.match(text.read(), /No downloads recorded yet/);

    for (const args of [['--json', '--json'], ['--json=1'], ['5']]) {
      await assert.rejects(historyMain(args, { root, stdout: sink().stdout }), /Usage: veo history \[--json\]/);
    }
    for (const args of [['--limit'], ['--limit', '0'], ['--limit', '1001'], ['--limit', '1.5']]) {
      await assert.rejects(historyMain(args, { root, stdout: sink().stdout }), /--limit must be/);
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('long file lists are summarized in the text view but complete in JSON', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-files-'));
  try {
    const files = Array.from({ length: 8 }, (_, index) => `C:/Videos/Entry ${index + 1}.mp4`);
    await createHistoryRecorder(root)({ url: 'https://example.test/list', title: 'A playlist', status: 'saved', files });
    const text = sink();
    await historyMain([], { root, stdout: text.stdout });
    assert.match(text.read(), /Saved:  C:\/Videos\/Entry 1\.mp4/);
    assert.match(text.read(), /… and 3 more file\(s\); use veo history --json for the full list\./);

    const json = sink();
    await historyMain(['--json'], { root, stdout: json.stdout });
    assert.equal(JSON.parse(json.read()).entries[0].files.length, 8);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('runJob records every finished item once, with its own outcome', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-job-'));
  try {
    const shared = { jobFile: path.join(root, 'job.json'), stdout: { write() {} }, stderr: { write() {} },
      reporter: { fail() {}, complete() {} }, recordHistory: createHistoryRecorder(root) };
    const saved = path.join(root, 'My Video.mp4');
    await runJob({ urls: ['https://example.test/ok'], output: root }, { ...shared, jobFile: path.join(root, 'ok.json'),
      download: async () => ({ status: 'saved', title: 'My \u001b[31mVideo', files: [saved] }) });
    await runJob({ urls: ['https://example.test/skip'], output: root }, { ...shared, jobFile: path.join(root, 'skip.json'),
      download: async () => ({ status: 'skipped', title: 'Skipped Video', files: [], skipped: 1 }) });
    await runJob({ urls: ['https://example.test/song'], output: root, audio: true }, { ...shared, jobFile: path.join(root, 'song.json'),
      download: async () => ({ status: 'saved', title: 'A Song', files: [path.join(root, 'A Song.mp3')] }) });
    await runJob({ urls: ['https://example.test/fail'], output: root }, { ...shared, jobFile: path.join(root, 'fail.json'),
      download: async () => { throw new Error('Requested format is not available'); } });

    const entries = await readHistory(root, 10);
    assert.deepEqual(entries.map(entry => entry.status), ['failed', 'saved', 'skipped', 'saved']);
    const [failed, song, skipped, ok] = entries;
    // A failed item has no title yet, so its URL is reported instead.
    assert.equal(failed.title, 'https://example.test/fail');
    assert.equal(failed.url, 'https://example.test/fail');
    assert.equal(failed.media, 'video');
    assert.match(failed.error, /format is unavailable/);
    assert.equal(failed.job, path.join(root, 'fail.json'));
    const display = sink();
    await historyMain(['--failed'], { root, stdout: display.stdout });
    assert.match(display.read(), /Retry:  veo --retry-failed/);
    assert.deepEqual(failed.files, []);
    assert.equal(song.media, 'audio');
    assert.equal(song.quality, null);
    assert.equal(song.format, 'mp3');
    assert.deepEqual(song.files, [path.join(root, 'A Song.mp3')]);
    assert.equal(skipped.status, 'skipped');
    assert.equal(ok.title, 'My Video', 'terminal escapes are stripped, not stored');
    assert.deepEqual(ok.files, [saved]);
    assert.ok(entries.every(entry => entry.elapsedMs >= 0));
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('history is kept by veo flush, including a reset of the statistics', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-flush-'));
  try {
    await createHistoryRecorder(root)({ url: 'https://example.test/v', title: 'Keep me', status: 'saved', files: [path.join(root, 'v.mp4')] });
    await flush({ root });
    await flush({ root, stats: true });
    assert.deepEqual((await readHistory(root)).map(entry => entry.title), ['Keep me']);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('unrelated files are ignored, a damaged or redirected directory is reported', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-history-invalid-'));
  try {
    const directory = historyRoot(root);
    await mkdir(directory, { recursive: true });
    await writeFile(path.join(directory, 'notes.txt'), 'not a record');
    await writeFile(path.join(directory, `${Date.now()}-${randomUUID()}.tmp`), '{}');
    assert.deepEqual(await readHistory(root), []);

    await writeFile(path.join(directory, `${Date.now()}-${randomUUID()}.json`), '{"version":1}');
    await assert.rejects(readHistory(root), /Invalid history file/);
  } finally { await rm(root, { recursive: true, force: true }); }

  const other = await mkdtemp(path.join(os.tmpdir(), 'veo-history-file-'));
  try {
    await writeFile(path.join(other, 'history'), 'a file where a directory belongs');
    await assert.rejects(readHistory(other), /Invalid history directory/);
  } finally { await rm(other, { recursive: true, force: true }); }
});
