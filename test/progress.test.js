import test from 'node:test';
import assert from 'node:assert/strict';
import { createReporter, formatProgress, terminalTitle } from '../src/progress.js';
import { outputStream } from '../src/output.js';
import { readableError } from '../src/utils.js';
import { runBackend } from '../src/downloader.js';
import { EventEmitter } from 'node:events';

const data = { stream: 'Media', downloaded_bytes: 90 * 1024 ** 2, total_bytes_estimate: 1024 ** 3, speed: 12.6 * 1024 ** 2, eta: 77 };

test('narrow progress keeps the bar and every statistic below it', () => {
  for (const columns of [10, 20, 30, 40, 60]) {
    const result = formatProgress(data, { columns, prefix: 'Media' });
    const joined = result.replace(/\n/g, '');
    assert.match(joined, /\[[=-]+\]/);
    assert.ok(joined.includes('~9%'));
    assert.ok(joined.includes('12.6 MiB/s'));
    assert.ok(joined.includes('90.0 MiB / ~1.0 GiB'));
    assert.ok(joined.includes('ETA 1:17'));
    for (const line of result.split('\n')) assert.ok(line.length <= columns);
    assert.ok(result.indexOf('[') < result.indexOf('12.6'));
  }
});

test('multiline updates erase all previous live rows without touching preceding logs', () => {
  const output = { isTTY: true, columns: 40, text: '', write(value) { this.text += value; } };
  const reporter = createReporter(output, { setTitle() {} });
  try {
    reporter.item(1, 1, 'Original title');
    reporter.progress(data);
    output.text = '';
    reporter.progress({ ...data, downloaded_bytes: 100 * 1024 ** 2 });
    assert.ok(output.text.startsWith('\r\x1b[2K\x1b[J'));
    assert.ok(!output.text.includes('Original title'));
    assert.ok(output.text.includes('100.0 MiB'));
  } finally { reporter.finish(); }
});

test('resize never rewrites prior titles and logs using guessed cursor positions', async () => {
  const output = { isTTY: true, columns: 120, text: '', write(value) { this.text += value; } };
  const reporter = createReporter(output, { setTitle() {} });
  reporter.configure({ color: false });
  const title = 'Title' + 'x'.repeat(200);
  try {
    reporter.item(1, 1, title);
    reporter.status('Size unknown; continuing.');
    output.text = '';
    output.columns = 40;
    await new Promise(resolve => setTimeout(resolve, 220));
    assert.equal(output.text, '');
    output.text = '';
    output.columns = 120;
    await new Promise(resolve => setTimeout(resolve, 220));
    assert.equal(output.text, '');
  } finally { reporter.finish(); }
});

test('size polling redraws without resize events and clears wrapped remnants before growing', async () => {
  const output = { isTTY: true, columns: 120, text: '', write(value) { this.text += value; } };
  const reporter = createReporter(output, { setTitle() {} });
  try {
    reporter.progress(data);
    output.text = '';
    output.columns = 30;
    await new Promise(resolve => setTimeout(resolve, 220));
    assert.ok(output.text.startsWith('\r\x1b[2K\x1b[J'));
    for (const line of output.text.split('\r\x1b[2K').at(-1).replace(/\x1b\[\d+A|\r$/g, '').split('\r\n')) assert.ok(line.length < 30);
    output.text = '';
    output.columns = 120;
    await new Promise(resolve => setTimeout(resolve, 220));
    assert.ok(output.text.includes('\x1b[J'));
    assert.ok(output.text.split('\r\x1b[2K').at(-1).includes('ETA'));
  } finally { reporter.finish(); }
  const finished = output.text;
  output.columns = 40;
  await new Promise(resolve => setTimeout(resolve, 220));
  assert.equal(output.text, finished);
});

test('resize redraws live progress without backend updates and restores fitting titles', () => {
  const output = Object.assign(new EventEmitter(), { isTTY: true, columns: 120, text: '', write(value) { this.text += value; } });
  const reporter = createReporter(outputStream(output), { setTitle() {} });
  const title = 'A moderately long video title';
  reporter.scoped(1, 2, title).progress(data);
  assert.ok(output.text.split('\r\x1b[2K').at(-1).includes(title));
  output.columns = 40;
  output.emit('resize');
  const narrow = output.text.split('\r\x1b[2K').at(-1);
  for (const line of narrow.split('\r\n')) assert.ok(line.length < 40);
  assert.ok(narrow.includes('…'));
  output.columns = 120;
  output.emit('resize');
  assert.ok(output.text.split('\r\x1b[2K').at(-1).includes(title));
  assert.equal(output.listenerCount('resize'), 1);
  reporter.finish();
  assert.equal(output.listenerCount('resize'), 0);
});

test('long URLs, titles and scoped status lines fit the terminal; pipe output stays complete', () => {
  const long = 'https://example.test/' + 'token'.repeat(100);
  for (const columns of [40, 80, 120]) {
    const output = { isTTY: true, columns, text: '', write(value) { this.text += value; } };
    const reporter = createReporter(output, { setTitle() {} });
    reporter.configure({ color: false });
    reporter.item(1, 2, long);
    reporter.name('Name ' + long);
    reporter.status(long);
    reporter.scoped(1, 2, long).status('Reading video: done');
    reporter.profile(long);
    const lines = output.text.trimEnd().split('\n');
    assert.equal(lines.length, 9);
    for (const line of lines) {
      assert.ok(line.length < columns, line);
    }
    for (const index of [2, 5, 6, 7, 8]) assert.ok(lines[index].endsWith('…'), lines[index]);
  }
  const pipe = { text: '', write(value) { this.text += value; } };
  createReporter(pipe, { setTitle() {} }).item(1, 1, long);
  assert.equal(pipe.text, long + '\n');
});

test('titles wrap to three lines and truncate only when that space is exceeded', () => {
  const terminal = { isTTY: true, columns: 11 };
  assert.equal(terminalTitle(terminal, 'Short'), 'Short');
  assert.equal(terminalTitle(terminal, 'a'.repeat(20)), 'a'.repeat(10) + '\n' + 'a'.repeat(10));
  assert.equal(terminalTitle(terminal, 'a'.repeat(30)), Array(3).fill('a'.repeat(10)).join('\n'));
  assert.equal(terminalTitle(terminal, 'a'.repeat(31)), ['a'.repeat(10), 'a'.repeat(10), 'a'.repeat(9) + '…'].join('\n'));
  assert.equal(terminalTitle(terminal, '界'.repeat(16)), ['界'.repeat(5), '界'.repeat(5), '界'.repeat(4) + '…'].join('\n'));
  terminal.columns = 40;
  assert.equal(terminalTitle(terminal, 'a'.repeat(31)), 'a'.repeat(31));
});

test('non-default profile is white after a gray label only when terminal colors are enabled', () => {
  const output = () => ({ isTTY: true, text: '', write(value) { this.text += value; } });
  const terminal = output();
  const reporter = createReporter(terminal, { setTitle() {}, env: {} });
  reporter.profile('kino');
  assert.equal(terminal.text, '\x1b[90mProfile: \x1b[0m\x1b[97mkino\x1b[0m\n');
  terminal.text = '';
  reporter.profile('default');
  assert.equal(terminal.text, '\x1b[90mProfile: default\x1b[0m\n');
  reporter.configure({ color: false });
  terminal.text = '';
  reporter.profile('kino');
  assert.equal(terminal.text, 'Profile: kino\n');
  const noColor = output();
  createReporter(noColor, { setTitle() {}, env: { NO_COLOR: '1' } }).profile('kino');
  assert.equal(noColor.text, 'Profile: kino\n');
  const pipe = output(); pipe.isTTY = false;
  createReporter(pipe, { setTitle() {}, env: {} }).profile('kino');
  assert.equal(pipe.text, 'Profile: kino\n');
});

test('Termux progress stays within the terminal and reuses one line across resizing', () => {
  const output = { isTTY: true, columns: 60, text: '', write(text) { this.text += text; } };
  const wrapped = outputStream(output);
  const reporter = createReporter(wrapped, { setTitle() {} });
  reporter.progress(data);
  for (const columns of [60, 40, 32, 20, 10, 80, 120]) {
    output.columns = columns;
    assert.equal(wrapped.columns, columns);
    for (let index = 0; index < 10; index++) reporter.progress({ ...data, downloaded_bytes: data.downloaded_bytes + index });
    const lastLine = output.text.split('\r\x1b[2K').at(-1);
    for (const line of lastLine.replace(/\x1b\[\d+A|\r$/g, '').split('\r\n')) assert.ok(line.length < columns, `${columns}: ${line}`);
  }
  reporter.finish();
  assert.ok(output.text.endsWith('\r\x1b[2K'));
});

test('estimated totals cannot display 100 percent; completed streams are not completed jobs', () => {
  const estimated = formatProgress({ downloaded_bytes: 1024, total_bytes_estimate: 1024 });
  assert.match(estimated, /~99%/);
  assert.match(estimated, /~1.0 KiB/);
  assert.doesNotMatch(estimated, /100%/);
  assert.match(formatProgress({ status: 'finished', downloaded_bytes: 1024, total_bytes: 1024 }), /received; processing/);
});

test('parallel progress shares a compact live line and handles wide titles', () => {
  const output = { isTTY: true, columns: 40, text: '', write(text) { this.text += text; } };
  const reporter = createReporter(output, { setTitle() {} });
  const first = reporter.scoped(1, 2, '🎥日本語'.repeat(20));
  const second = reporter.scoped(2, 2, 'Second');
  for (let index = 0; index < 20; index++) { first.progress(data); second.progress(data); }
  for (const frame of output.text.split('\r\x1b[2K').filter(Boolean)) for (const line of frame.replace(/\x1b\[[0-9;]*[AJ]/g, '').split('\r\n')) assert.ok(line.length < 40);
  assert.match(output.text, /\[1\/2\]/);
  assert.match(output.text, /\[2\/2\]/);
});

test('redirected output remains throttled and contains no terminal control codes', () => {
  const output = { text: '', write(text) { this.text += text; } };
  const reporter = createReporter(output, { setTitle() {} });
  for (let index = 0; index < 20; index++) reporter.progress(data);
  assert.equal(output.text.split('\n').filter(Boolean).length, 2);
  assert.doesNotMatch(output.text, /\x1b|\r/);
});

test('waiting status animates in place and processing replaces it with a colored result', async () => {
  const output = { isTTY: true, columns: 80, text: '', write(text) { this.text += text; } };
  const reporter = createReporter(output, { setTitle() {}, env: {} });
  reporter.status('Reading video…');
  await new Promise(resolve => setTimeout(resolve, 380));
  assert.match(output.text, /Reading video\./);
  assert.match(output.text, /Reading video\.\./);
  assert.equal(output.text.includes('\n'), false);
  reporter.processing({ postprocessor: 'MoveFiles', status: 'started' });
  assert.equal(output.text.includes('\n'), false);
  reporter.processing({ postprocessor: 'MoveFiles', status: 'finished' });
  assert.match(output.text, /Preparing saved file: \x1b\[0m\x1b\[32mdone\x1b\[0m\n$/);
  const settled = output.text;
  await new Promise(resolve => setTimeout(resolve, 380));
  assert.equal(output.text, settled);
});

test('failed processing is red with color and plain with no color', () => {
  for (const color of [true, false]) {
    const output = { isTTY: true, columns: 80, text: '', write(text) { this.text += text; } };
    const reporter = createReporter(output, { setTitle() {}, env: {} });
    reporter.configure({ color });
    reporter.processing({ postprocessor: 'MoveFiles', status: 'started' });
    reporter.processing({ postprocessor: 'MoveFiles', status: 'failed' });
    assert.ok(output.text.endsWith(color ? 'Preparing saved file: \x1b[0m\x1b[31mfailed\x1b[0m\n' : 'Preparing saved file: failed\n'));
    reporter.complete();
  }
});

test('an active save failure replaces its animation without affecting another scoped item', () => {
  const output = { isTTY: true, columns: 80, text: '', write(text) { this.text += text; } };
  const reporter = createReporter(output, { setTitle() {}, env: {} });
  const first = reporter.scoped(1, 2, 'First');
  const second = reporter.scoped(2, 2, 'Second');
  first.status('Reading video…');
  second.processing({ postprocessor: 'MoveFiles', status: 'started' });
  first.failStep();
  assert.equal(output.text.includes('failed'), false);
  second.failStep();
  assert.match(output.text, /Second: Preparing saved file: \x1b\[0m\x1b\[31mfailed\x1b\[0m\n$/);
  reporter.complete();
});

test('completed yt-dlp check and video read remain as separate lines', () => {
  const output = { isTTY: true, columns: 80, text: '', write(text) { this.text += text; } };
  const reporter = createReporter(output, { setTitle() {}, env: {} });
  reporter.configure({ color: false });
  reporter.status('Checking yt-dlp…');
  reporter.finishStatus('Checking yt-dlp…', 'done');
  reporter.status('Reading video…');
  reporter.finishStatus('Reading video…', 'done');
  const lines = output.text.trimEnd().split('\n');
  assert.equal(lines.length, 2);
  assert.ok(lines[0].endsWith('Checking yt-dlp: done'));
  assert.ok(lines[1].endsWith('Reading video: done'));
  reporter.status('Downloading…');
  assert.ok(output.text.includes('Checking yt-dlp: done\n'));
  assert.ok(output.text.includes('Reading video: done\n'));
  reporter.finish();
});

test('yt-dlp check and video read use success and error colors when enabled', () => {
  for (const label of ['Checking yt-dlp', 'Reading video']) for (const outcome of ['done', 'failed']) {
    const output = { isTTY: true, columns: 80, text: '', write(text) { this.text += text; } };
    const reporter = createReporter(output, { setTitle() {}, env: {} });
    reporter.status(`${label}…`);
    reporter.finishStatus(`${label}…`, outcome);
    assert.ok(output.text.endsWith(`${label}: \x1b[0m\x1b[${outcome === 'done' ? 32 : 31}m${outcome}\x1b[0m\n`));
    reporter.finish();
  }
});

test('download errors select the final backend failure and retain HTTP status', async () => {
  await assert.rejects(runBackend(process.execPath, ['-e', 'console.error("WARNING: cookies were not needed"); console.error("ERROR: unable to download video data: HTTP Error 403: Forbidden"); console.error("veo-progress:{}"); process.exit(1)']), error => {
    assert.match(readableError(error), /HTTP 403 Forbidden/);
    assert.doesNotMatch(error.message, /veo-progress|WARNING/);
    return true;
  });
  assert.match(readableError(new Error('HTTP Error 429: Too Many Requests')), /429.*rate-limiting/);
  assert.match(readableError(new Error('HTTP Error 503: Service Unavailable')), /HTTP 503/);
  assert.match(readableError(new Error('read ECONNRESET')), /ECONNRESET/);
});
