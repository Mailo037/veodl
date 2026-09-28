import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { parseCli } from '../src/cli.js';
import { loadConfig } from '../src/config.js';
import { download as actualDownload, localRequestKey, partialKey, planDownload } from '../src/downloader.js';
import { interactiveArgs } from '../src/interactive.js';
import { latestFailedJob, retryOptions, runJob } from '../src/jobs.js';
import { registerRun } from '../src/runs.js';
import { createReporter } from '../src/progress.js';
import { selectedEntries, sizeEstimate, validateItems } from '../src/playlist.js';

const download = (options, dependencies = {}) => actualDownload(options, { localRoot: options.output, ...dependencies });

const url = 'https://example.test/list';
const backendResolver = async () => ({ ytDlp: 'fake', ffmpegLocation: 'tools' });
const sink = () => ({ text: '', write(value) { this.text += value; } });
const reporter = () => createReporter(sink(), { setTitle() {} });

test('incognito saves media but leaves no run, retry, history, stats or staging data', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-incognito-test-'));
  const jobFile = path.join(root, 'retry.json');
  const stdout = sink(), stderr = sink();
  let staged;
  let backendCalls = 0;
  let recorderCalls = 0;
  const runner = async (_, args, { onLine } = {}) => {
    backendCalls++;
    assert.ok(args.includes('--no-cache-dir'));
    if (args.includes('--dump-single-json')) return JSON.stringify({ id: 'private', title: 'Private video', formats: [{ height: 720, vcodec: 'h264' }] });
    const file = path.join(path.dirname(args[args.indexOf('-o') + 1]), 'media.mp4');
    await writeFile(file, 'private media');
    onLine?.(`veo-file:${JSON.stringify(file)}`);
    return '';
  };
  try {
    const options = { urls: [url], output: root, incognito: true, json: true };
    assert.equal(await runJob(options, { reporter: reporter(), stdout, stderr, jobFile, runId: 'abc123', archiveRoot: root,
      recordHistory: async () => { recorderCalls++; }, recordStats: async () => { recorderCalls++; },
      download: (request, dependencies) => {
        staged = dependencies.localRoot;
        return actualDownload(request, { ...dependencies, backendResolver, runner });
      } }), 0);
    const result = JSON.parse(stdout.text.trim());
    assert.equal(result.status, 'saved');
    assert.equal(Object.hasOwn(result, 'runId'), false);
    assert.equal(await readFile(result.files[0], 'utf8'), 'private media');
    assert.deepEqual(await readdir(root), ['Private video.mp4']);
    assert.equal(recorderCalls, 0);
    assert.ok(backendCalls >= 2);
    await assert.rejects(readdir(staged), { code: 'ENOENT' });
    assert.doesNotMatch(stderr.text, /Run ID|Retry failed/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('incognito failure removes temporary data and offers no retry command', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-incognito-fail-'));
  const stderr = sink();
  let staged;
  try {
    const code = await runJob({ urls: [url], output: root, incognito: true }, {
      reporter: reporter(), stdout: sink(), stderr, jobFile: path.join(root, 'job.json'),
      download: async (_request, dependencies) => {
        staged = dependencies.localRoot;
        await writeFile(path.join(staged, 'unfinished.part'), 'partial media');
        throw new Error('backend failed');
      },
    });
    assert.equal(code, 1);
    assert.deepEqual(await readdir(root), []);
    await assert.rejects(readdir(staged), { code: 'ENOENT' });
    assert.doesNotMatch(stderr.text, /Retry failed|Run ID/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('optional neutral filename hides title, custom name and template folders', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-neutral-name-'));
  const runner = async (_, args, { onLine } = {}) => {
    if (args.includes('--dump-single-json')) return JSON.stringify({ id: 'identifying-id', title: 'Private title', channel: 'Private channel', formats: [{ height: 720, vcodec: 'h264' }] });
    const file = path.join(path.dirname(args[args.indexOf('-o') + 1]), 'media.mp4');
    await writeFile(file, 'media');
    onLine?.(`veo-file:${JSON.stringify(file)}`);
    return '';
  };
  try {
    const result = await actualDownload({ url, output: root, incognito: true, neutralFilename: true,
      rename: 'Personal label', filenameTemplate: '{title}', folderTemplate: '{channel}/{playlist}' },
    { localRoot: path.join(root, 'staging'), backendResolver, runner });
    assert.match(path.basename(result.files[0]), /^video-[a-f0-9]{12}\.mp4$/);
    assert.equal(path.dirname(result.files[0]), root);
    assert.doesNotMatch(result.files[0], /Private|Personal|identifying/);
    assert.deepEqual((await readdir(root)).filter(name => name !== 'staging'), [path.basename(result.files[0])]);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('default profile applies automatically, explicit profiles replace it and flags win', async () => {
  const config = { open: true, profiles: { default: { quality: '720p', output: './everyday' }, music: { audio: true, format: 'mp3' } } };
  assert.equal(parseCli([url], { config }).quality, '720p');
  assert.equal(parseCli([url], { config }).profile, 'default');
  assert.equal(parseCli([url, '--profile', 'default'], { config }).output, './everyday');
  assert.equal(parseCli([url, '-q', '1080p'], { config }).quality, '1080p');
  assert.equal(parseCli([url, '--no-open'], { config }).open, false);
  const music = parseCli([url, '--profile', 'music'], { config });
  assert.equal(music.audio, true);
  assert.equal(music.quality, 'best');
  assert.equal(music.open, true);
  const answers = ['', url, 'video', '', '', 'y', 'y'];
  const prompts = [];
  const args = await interactiveArgs(config, { output: sink(), ask: async prompt => { prompts.push(prompt); return answers.shift(); }, inspect: async () => ({ formats: [{ height: 720, vcodec: 'h264' }] }) });
  assert.match(prompts[0], /\[default\]/);
  assert.equal(parseCli(args, { config }).quality, '720p');
  assert.equal(parseCli(args, { config }).output, './everyday');
});

test('profiles merge defaults, explicit flags override and booleans can be disabled', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-profile-'));
  try {
    const file = path.join(directory, 'config.json');
    await writeFile(file, JSON.stringify({ quality: '720p', open: true, profiles: { music: { audio: true, format: 'mp3' }, archive: { embedMetadata: true, subLangs: 'de,en' } } }));
    const { config } = await loadConfig({ file });
    const music = parseCli([url, '--profile', 'music', '--no-open'], { config });
    assert.equal(music.audio, true);
    assert.equal(music.open, false);
    const video = parseCli([url, '--profile=archive', '-q', '1080p', '--no-embed-metadata', '--no-subs'], { config });
    assert.equal(video.quality, '1080p');
    assert.equal(video.embedMetadata, false);
    assert.equal(video.subs, false);
    assert.equal(video.subLangs, undefined);
    assert.throws(() => parseCli([url, '--profile', 'missing'], { config }), /Unknown profile/);
    await writeFile(file, JSON.stringify({ profiles: { bad: { audio: 'yes' } } }));
    await assert.rejects(loadConfig({ file }), /must be a boolean/);
    // Explicit flags count as explicit even when they equal the stored default.
    assert.throws(() => parseCli([url, '--audio', '-q', '720p'], { config: { quality: '720p', audio: true } }), /--quality is for video/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('playlist ranges are bounded selections and report partial size estimates honestly', () => {
  const metadata = { entries: [{ filesize: 100 }, {}, { filesize_approx: 200 }, {}] };
  assert.deepEqual(selectedEntries(metadata, '1,3-4').map(item => item.index), [1, 3, 4]);
  assert.deepEqual(sizeEstimate(selectedEntries(metadata)), { estimatedBytes: 300, knownSizes: 2, totalEntries: 4 });
  for (const bad of ['0', '-1', '3-1', '1,', 'all', '1:3', '999999999999999999']) assert.throws(() => validateItems(bad));
  assert.throws(() => selectedEntries(metadata, '9'), /No playlist entries/);
  assert.equal(parseCli([url, '--playlist-items', '1,3-5']).playlist, true);
  assert.equal(parseCli([url, '--no-playlist'], { config: { playlist: true, playlistItems: '1' } }).playlist, false);
});

test('interactive wizard offers source resolutions, returns valid flags and cancels cleanly', async () => {
  const output = sink();
  const answers = [url, 'video', '999p', '720p', './downloads', 'y', 'y'];
  const args = await interactiveArgs({}, { output, ask: async () => answers.shift(), inspect: async () => ({ title: 'Test', formats: [{ height: 720, vcodec: 'h264' }] }) });
  const parsed = parseCli(args);
  assert.equal(parsed.quality, '720p');
  assert.equal(parsed.resume, true);
  assert.equal(parsed.skipExisting, true);
  assert.match(output.text, /Choose: best, 720p/);
  const cancelAnswers = [url, 'audio', '', 'n', 'n'];
  assert.equal(await interactiveArgs({}, { output, ask: async () => cancelAnswers.shift(), inspect: async () => ({ title: 'Test' }) }), null);
});

test('interactive wizard detects collections and uses profile playlist selection', async () => {
  const prompts = [];
  const config = { profiles: { default: { playlist: true, playlistItems: '2', audio: true } } };
  const answers = ['', url, '', '', '', '', 'y', 'y'];
  const args = await interactiveArgs(config, { output: sink(), ask: async prompt => { prompts.push(prompt); return answers.shift(); },
    inspect: async options => {
      assert.equal(options.playlist, true);
      return { title: 'Collection', entries: [{ title: 'One' }, { title: 'Two' }] };
    } });
  const parsed = parseCli(args, { config });
  assert.equal(parsed.playlist, true);
  assert.equal(parsed.playlistItems, '2');
  assert.equal(parsed.audio, true);
  assert.match(prompts.find(prompt => prompt.startsWith('Download the playlist')), /\[y\]/);
  assert.match(prompts.find(prompt => prompt.startsWith('Entries')), /Enter = 2/);
});

test('interactive wizard honors an incognito profile without offering history based skip', async () => {
  const config = { profiles: { private: { incognito: true, resume: true, skipExisting: true } }, activeProfile: 'private' };
  const output = sink();
  const answers = ['', url, 'video', '', '', '', 'y'];
  const args = await interactiveArgs(config, { output, ask: async () => answers.shift(),
    inspect: async () => ({ title: 'Private', formats: [{ height: 720, vcodec: 'h264' }] }) });
  const parsed = parseCli(args, { config });
  assert.equal(parsed.incognito, true);
  assert.equal(parsed.resume, false);
  assert.equal(parsed.skipExisting, false);
  assert.doesNotMatch(output.text, /Skip previously downloaded/);
});

test('latest failed retry job skips newer successful and active jobs', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-retry-last-'));
  try {
    const jobs = path.join(root, 'jobs');
    await mkdir(jobs);
    const name = stamp => `${stamp}-${'a'.repeat(36)}.json`;
    const failed = path.join(jobs, name('1700000000001'));
    const completed = path.join(jobs, name('1700000000002'));
    const active = path.join(jobs, name('1700000000003'));
    const options = { output: root };
    await writeFile(failed, JSON.stringify({ version: 1, options, items: [{ status: 'failed', url }] }));
    await writeFile(completed, JSON.stringify({ version: 1, options, items: [{ status: 'saved', url }] }));
    await writeFile(active, JSON.stringify({ version: 1, options, items: [{ status: 'running', url }] }));
    const run = await registerRun(() => {}, root);
    try {
      await run.describe({ job: active });
      assert.equal(await latestFailedJob(root), failed);
      assert.equal((await retryOptions(await latestFailedJob(root))).length, 1);
    } finally { await run.unregister(); }
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('retry by run ID resolves existing jobs and rejects active, completed and missing runs', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-retry-id-'));
  const jobs = path.join(root, 'jobs');
  try {
    await mkdir(jobs);
    const file = path.join(jobs, '1700000000001-00000000-0000-0000-0000-000000000000.json');
    const job = { version: 1, runId: 'abc123', options: { output: root, quality: '720p' }, items: [{ status: 'failed', url }, { status: 'saved', url: 'https://example.test/saved' }] };
    await writeFile(file, JSON.stringify(job));
    assert.equal(parseCli(['--retry-failed', 'abc123']).retryFailed, 'abc123');
    assert.deepEqual(await retryOptions('abc123', root), await retryOptions(file));
    await assert.rejects(retryOptions('xyz789', root), /No retry job found for run xyz789/);
    const run = await registerRun(() => {}, root);
    try {
      await run.describe({ job: file });
      job.runId = run.id;
      await writeFile(file, JSON.stringify(job));
      await assert.rejects(retryOptions(run.id, root), /still active/);
    } finally { await run.unregister(); }
    job.items = [{ status: 'saved', url }];
    await writeFile(file, JSON.stringify(job));
    await assert.rejects(retryOptions(job.runId, root), /no failed or unfinished/);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('playlist failure preserves successes and resume downloads only the missing entry', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-list-resume-'));
  const attempted = [];
  let fail = true;
  const entries = [1, 2, 3].map(index => ({ id: `v${index}`, title: `Video ${index}` }));
  const runner = async (_, args, { onLine } = {}) => {
    const index = Number(args[args.indexOf('--playlist-items') + 1]);
    if (args.includes('--dump-single-json')) return JSON.stringify(index ? { ...entries[index - 1], extractor_key: 'Test', formats: [] } : { id: 'list', title: 'List', entries });
    attempted.push(index);
    const file = path.join(path.dirname(args[args.indexOf('-o') + 1]), 'media.mp4');
    if (index === 2 && fail) { await writeFile(`${file}.part`, 'partial'); throw new Error('temporary failure'); }
    await writeFile(file, `video ${index}`);
    onLine(`veo-file:${JSON.stringify(file)}`);
  };
  const options = { url, output: directory, quality: 'best', playlist: true, resume: true, adaptiveConcurrency: false };
  try {
    const first = await download(options, { backendResolver, runner });
    assert.equal(first.saved, 2);
    assert.deepEqual(first.failures.map(item => item.index), [2]);
    assert.equal(await readFile(path.join(directory, 'Video 1.mp4'), 'utf8'), 'video 1');
    fail = false;
    const second = await download(options, { backendResolver, runner });
    assert.equal(second.saved, 1);
    assert.equal(second.skipped, 2);
    assert.deepEqual(attempted.slice(0, 3).sort(), [1, 2, 3]);
    assert.deepEqual(attempted.slice(3), [2]);
    assert.equal((await readdir(directory)).filter(name => name.startsWith('.veo-part')).length, 0);
    await rm(path.join(directory, 'Video 1.mp4'));
    const third = await download({ ...options, playlistItems: '1', skipExisting: true }, { backendResolver, runner });
    assert.equal(third.saved, 1, 'deleted output must be downloaded again');
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('jobs open successful files despite failure and retry only failed URLs with stable paths', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-job-'));
  const stdout = sink(), stderr = sink(), opened = [];
  const jobFile = path.join(directory, 'job.json');
  try {
    const options = { urls: [url, `${url}/bad`, `${url}/last`], output: './relative', quality: '720p', open: true, cookies: 'secret-file', cookiesFromBrowser: 'firefox' };
    const code = await runJob(options, { reporter: reporter(), stdout, stderr, jobFile,
      download: async request => { if (request.url.endsWith('/bad')) throw new Error('failed'); return { url: request.url, files: [`${directory}/${request.url.endsWith('/last') ? 'last' : 'first'}.mp4`] }; },
      openFile: async file => opened.push(file) });
    assert.equal(code, 1);
    assert.equal(opened.length, 2);
    assert.match(stderr.text, /2 saved, 0 skipped, 1 failed/);
    const retry = await retryOptions(jobFile);
    assert.deepEqual(retry.map(item => item.url), [`${url}/bad`]);
    assert.equal(retry[0].quality, '720p');
    assert.equal(retry[0].output, path.resolve('./relative'));
    const saved = await readFile(jobFile, 'utf8');
    assert.doesNotMatch(saved, /secret-file|cookiesFromBrowser|firefox/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('single downloads show timing without a summary; multiple downloads show both', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-timing-summary-'));
  const first = { setup: 1000, metadata: 2000 };
  const second = { setup: 2000, metadata: 3000 };
  try {
    const single = sink();
    await runJob({ urls: [url], output: directory }, { reporter: reporter(), stderr: single, stdout: sink(),
      jobFile: path.join(directory, 'single.json'), download: async () => ({ status: 'saved', files: [], timings: first }) });
    assert.doesNotMatch(single.text, /Summary:/);
    assert.match(single.text, /Timing: setup 1\.0s \| metadata 2\.0s\n/);

    const multiple = sink();
    await runJob({ urls: [url, `${url}/second`], output: directory, concurrentDownloads: 1 }, {
      reporter: reporter(), stderr: multiple, stdout: sink(), jobFile: path.join(directory, 'multiple.json'),
      download: async request => ({ status: 'saved', files: [], timings: request.url === url ? first : second }),
    });
    assert.match(multiple.text, /Summary: 2 saved, 0 skipped, 0 failed\.\nTiming: setup 3\.0s \| metadata 5\.0s\n/);

    const playlist = sink();
    await runJob({ urls: [url], output: directory, playlist: true }, { reporter: reporter(), stderr: playlist, stdout: sink(),
      jobFile: path.join(directory, 'playlist.json'), download: async (_, { onEntry }) => {
        await onEntry({ index: 1, status: 'saved', files: [], timings: first });
        await onEntry({ index: 2, status: 'saved', files: [], timings: second });
        return { status: 'saved', saved: 2, files: [], entryTimings: [{ index: 1, timings: first }, { index: 2, timings: second }] };
      } });
    assert.match(playlist.text, /Summary: 2 saved, 0 skipped, 0 failed\.\nTiming: setup 3\.0s \| metadata 5\.0s\n/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('cancelled jobs retain completed playlist outputs and retry unfinished work', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-cancel-job-'));
  const stdout = sink(), stderr = sink(), controller = new AbortController();
  const jobFile = path.join(directory, 'job.json');
  try {
    const code = await runJob({ urls: [url, `${url}/next`], output: directory, json: true, profile: 'music', playlist: true, concurrentDownloads: 1 }, {
      reporter: reporter(), stdout, stderr, jobFile, signal: controller.signal,
      download: async (_, { onEntry }) => {
        await onEntry({ index: 1, status: 'saved', files: ['first.mp4'] });
        controller.abort();
        throw new DOMException('Cancelled', 'AbortError');
      } });
    assert.equal(code, 130);
    assert.equal(JSON.parse(stdout.text.trim()).status, 'cancelled');
    assert.equal(JSON.parse(stdout.text.trim()).profile, 'music');
    assert.deepEqual(JSON.parse(stdout.text.trim()).files, ['first.mp4']);
    assert.match(stderr.text, /1 saved, 0 skipped, 0 failed/);
    assert.equal((await retryOptions(jobFile)).length, 2);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('dry run reserves duplicate names and respects playlist selection and rename', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-plan-items-'));
  try {
    const metadata = { entries: [{ title: 'Same' }, { title: 'Same' }, { title: 'Last' }] };
    const plan = await planDownload({ url, output: directory, playlist: true, playlistItems: '1-2' }, { backendResolver, runner: async () => JSON.stringify(metadata) });
    assert.deepEqual(plan.entries.map(entry => path.basename(entry.path)), ['Same.mp4', 'Same (1).mp4']);
    const named = await planDownload({ url, output: directory, playlist: true, playlistItems: '3', rename: 'Custom' }, { backendResolver, runner: async () => JSON.stringify(metadata) });
      assert.equal(path.basename(named.entries[0].path), 'Custom - 003.mp4');
      const wildcard = await planDownload({ url, output: directory, playlist: true, playlistItems: '1-2', rename: 'movie_*' }, { backendResolver, runner: async () => JSON.stringify(metadata) });
      assert.deepEqual(wildcard.entries.map(entry => path.basename(entry.path)), ['movie_Same.mp4', 'movie_Same (1).mp4']);
    assert.deepEqual(await readdir(directory), []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('stream and conversion phases identify progress resets', () => {
  const output = sink();
  const progress = createReporter(output, { setTitle() {} });
  progress.item(3, 12, 'A video');
  progress.progress({ stream: 'Video', status: 'finished', downloaded_bytes: 100, total_bytes: 100 });
  progress.progress({ stream: 'Audio', downloaded_bytes: 20, total_bytes: 100 });
  progress.processing({ postprocessor: 'Merger', status: 'started' });
  progress.processing({ postprocessor: 'VideoConvertor', status: 'started' });
  assert.match(output.text, /\[3\/12\] A video/);
  assert.match(output.text, /Video download/);
  assert.match(output.text, /Audio download/);
  assert.match(output.text, /Merging audio\/video/);
  assert.match(output.text, /Converting video/);
});

test('unconfirmed media is sent through backend processing instead of blindly marked complete', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-unconfirmed-'));
  const options = { url, output: directory, resume: true, quality: 'best', format: 'webm' };
  const metadata = { id: 'video', title: 'Video' };
  let passes = 0;
  try {
    const runner = async (_, args, { onLine } = {}) => {
      if (args.includes('--dump-single-json')) return JSON.stringify(metadata);
      passes++;
      const stage = path.dirname(args[args.indexOf('-o') + 1]);
      if (passes === 1) { await writeFile(path.join(stage, 'media.mp4'), 'not-converted'); throw new Error('conversion interrupted'); }
      const file = path.join(stage, 'media.webm');
      await writeFile(file, 'converted');
      onLine(`veo-file:${JSON.stringify(file)}`);
    };
    await assert.rejects(download(options, { backendResolver, runner }), /conversion interrupted/);
    const result = await download(options, { backendResolver, runner });
    assert.equal(passes, 2);
    assert.equal(await readFile(result.files[0], 'utf8'), 'converted');
    assert.equal(path.extname(result.files[0]), '.webm');
    assert.notEqual(partialKey(metadata, url, options), partialKey(metadata, url, { ...options, audio: true }));
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('a failure recording history can resume the saved media and sidecars without duplicates', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-save-resume-'));
  const options = { url, output: directory, quality: 'best', resume: true, subs: true };
  let downloads = 0;
  const runner = async (_, args, { onLine } = {}) => {
    if (args.includes('--dump-single-json')) return JSON.stringify({ id: 'v', title: 'Video' });
    downloads++;
    const stage = path.dirname(args[args.indexOf('-o') + 1]);
    await writeFile(path.join(stage, 'media.mp4'), 'media');
    await writeFile(path.join(stage, 'media.en.vtt'), 'subtitles');
    onLine(`veo-file:${JSON.stringify(path.join(stage, 'media.mp4'))}`);
  };
  try {
    await writeFile(path.join(directory, '.veo-history'), 'blocking file');
    await assert.rejects(download(options, { backendResolver, runner }));
    await rm(path.join(directory, '.veo-history'));
    const result = await download(options, { backendResolver, runner });
    assert.equal(downloads, 1);
    assert.deepEqual(result.files.map(file => path.basename(file)), ['Video.mp4', 'Video.en.vtt']);
    assert.equal((await readdir(directory)).filter(name => name.includes('(1)')).length, 0);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
