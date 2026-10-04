import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { HELP, HELP_ALL, cliOptions } from '../src/cli.js';
import { formatOutput } from '../src/output.js';
import { formatProgress, pathRows, wordWrap, supportsUnicode } from '../src/progress.js';
import { formatChanges, parseChangelog, releaseCounts } from '../src/changes.js';

const run = promisify(execFile);
const tty = (columns = 80) => ({ isTTY: true, columns, text: '', write(value) { this.text += value; } });
const plain = text => text.replace(/\x1b\[[0-9;]*m/g, '');
const colors = !Object.hasOwn(process.env, 'NO_COLOR') && process.env.TERM !== 'dumb';

test('every help description starts in the same column', () => {
  for (const help of [HELP, HELP_ALL]) {
    const columns = new Set();
    for (const line of help.split('\n')) {
      const row = /^  (-\S.*?|veo .*?)  +(\S)/.exec(line);
      if (row) columns.add(row.index + row[0].length - 1);
      else if (/^ {3,}\S/.test(line)) columns.add(line.length - line.trimStart().length);
    }
    assert.deepEqual([...columns], [30], help);
  }
});

test('short help points to help all, which documents every option', async () => {
  assert.match(HELP, /veo help all/);
  assert.ok(HELP.split('\n').length < HELP_ALL.split('\n').length / 2);
  for (const name of Object.keys(cliOptions())) {
    if (name !== 'color') assert.ok(HELP_ALL.includes(`--${name}`), name);
  }
  const cli = fileURLToPath(new URL('../bin/veo.js', import.meta.url));
  for (const args of [['help', 'all'], ['--help', 'all']]) {
    const { stdout } = await run(process.execPath, [cli, ...args], { env: { ...process.env, NO_COLOR: '1' } });
    assert.equal(stdout, HELP_ALL);
  }
  assert.equal((await run(process.execPath, [cli, '--help'])).stdout, HELP);});

test('README usage block matches veo help all', async () => {
  const readme = await readFile(new URL('../README.md', import.meta.url), 'utf8');
  const block = readme.slice(readme.indexOf('```text\n', readme.indexOf('## Usage')) + 8);
  const expected = HELP_ALL.slice(HELP_ALL.indexOf('Usage:'), HELP_ALL.indexOf('\nRun veo without')).trimEnd();
  assert.equal(block.slice(0, block.indexOf('```')).trimEnd(), expected);
});

test('changes lists sections in canonical order and never styles a bare list row as a heading', () => {
  const releases = parseChangelog('# Changelog\n\n## 1.1.0\n\n### Fixed\n- a\n\n### Added\n- b\n\n### Changed\n- c\n\n## 1.0.0\n');
  assert.deepEqual(Object.keys(releaseCounts(releases[0])), ['Added', 'Changed', 'Fixed']);
  const list = formatChanges({ mode: 'list', versions: releases }, { current: '1.1.0' });
  assert.match(list, /1\.1\.0 {2}\(installed\) {2}Added 1, Changed 1, Fixed 1/);
  assert.ok(list.split('\n').every(line => line === line.trimEnd()), list);
  if (colors) assert.doesNotMatch(formatOutput(list, tty()), /\x1b\[1m {2}1\.0\.0/);
  const single = formatChanges({ mode: 'version', from: '1.1.0', versions: [releases[0]] });
  assert.ok(single.indexOf('Added') < single.indexOf('Changed') && single.indexOf('Changed') < single.indexOf('Fixed'));
});

test('word wrap breaks between words and splits only words wider than the line', () => {
  assert.equal(wordWrap('verify compressed and executable hashes', 20), 'verify compressed\nand executable\nhashes');
  assert.equal(wordWrap('abcdefghij', 4), 'abcd\nefgh\nij');
  assert.equal(wordWrap('short', Infinity), 'short');
});

test('doctor rows color only the status word; warnings are yellow', { skip: !colors }, () => {
  const out = formatOutput('  ok    Node.js           v22\n  warn  yt-dlp            not cached\n  fail  Backend           missing\nNo problems found, 1 warning.\n', tty());
  assert.match(out, /^ {2}\x1b\[32mok\x1b\[0m {4}\x1b\[90mNode\.js/m);
  assert.match(out, /^ {2}\x1b\[33mwarn\x1b\[0m {2}yt-dlp/m);
  assert.match(out, /^ {2}\x1b\[31mfail\x1b\[0m {2}Backend/m);
  assert.match(out, /\x1b\[33mNo problems found, 1 warning\./);
  assert.match(formatOutput('No problems found.\n', tty()), /\x1b\[32mNo problems found/);
});

test('stats labels are muted, values plain, failures red', { skip: !colors }, () => {
  const out = formatOutput('Videos saved:   3\nTotal failures: 2\nSkipped:        0\n', tty());
  assert.match(out, /\x1b\[90mVideos saved:\x1b\[0m {3}3\n/);
  assert.match(out, /\x1b\[90mTotal failures:\x1b\[0m \x1b\[31m2\x1b\[0m/);
});

test('unicode progress uses block glyphs, fixed-width fields and an indeterminate bar', () => {
  const bar = formatProgress({ downloaded_bytes: 512, total_bytes: 1024, speed: 1024, eta: 1 }, { unicode: true });
  assert.equal(bar, '██████████░░░░░░░░░░  50%   1.0 KiB/s      512 B / 1.0 KiB  ETA 0:01');
  const later = formatProgress({ downloaded_bytes: 1000, total_bytes: 1024, speed: 1024 * 1024 * 10, eta: 0 }, { unicode: true });
  assert.equal(later.indexOf('ETA'), bar.indexOf('ETA'));
  const frames = [0, 4, 8].map(frame => formatProgress({ downloaded_bytes: 2048, speed: 1024 }, { unicode: true, frame }));
  for (const frame of frames) assert.match(frame, /^░*███░* {3}\?% .*2\.0 KiB received/);
  assert.equal(new Set(frames).size, 3);
  const painted = formatProgress({ downloaded_bytes: 512, total_bytes: 1024 }, { unicode: true, paint: (text, role) => `<${role}>${text}` });
  assert.match(painted, /^<accent>█{10}<muted>░{10}/);
  assert.match(formatProgress({ downloaded_bytes: 512, total_bytes: 1024 }), /^\[={10}-{10}\]/);
  assert.equal(supportsUnicode({ isTTY: true }, { LANG: 'de_DE.UTF-8' }), true);
  assert.equal(supportsUnicode({ isTTY: true }, { LANG: 'C' }), false);
  assert.equal(supportsUnicode({ isTTY: true }, { TERM: 'dumb' }), false);
  assert.equal(supportsUnicode({ isTTY: false }, {}), false);
});

test('saved paths wrap at separators instead of losing the file name', () => {
  const target = '/home/user/videos/' + 'channel-name/'.repeat(5) + 'A long video title.mp4';
  const rows = pathRows(tty(40), 'Saved: ', target);
  assert.equal(rows.join(''), target);
  for (const row of rows) assert.ok(('Saved: ' + row).length < 40, row);
  assert.ok(rows.slice(0, -1).every(row => row.endsWith('/')), rows.join('\n'));
  assert.deepEqual(pathRows({ isTTY: false }, 'Saved: ', target), [target]);
  assert.equal(plain(formatOutput(`Saved: ${rows[0]}\n`, tty(40))), `Saved: ${rows[0]}\n`);
});
