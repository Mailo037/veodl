import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { folderLink, supportsPathLinks, linkOutputPaths } from '../src/path-links.js';
import { outputStream } from '../src/output.js';
import { historyMain, createHistoryRecorder } from '../src/history.js';

test('existing files link to their folder with encoded paths and a shortened label', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo links '));
  try {
    const file = path.join(root, 'media #1.mp4');
    await writeFile(file, 'fixture');
    const env = { WT_SESSION: 'test' };
    const linked = folderLink({ isTTY: true }, file, 'media…', env);
    assert.equal(linked, `\x1b]8;;${pathToFileURL(root + path.sep).href}\x1b\\media…\x1b]8;;\x1b\\`);
    assert.equal(folderLink({ isTTY: true }, root, root, env).includes(pathToFileURL(root + path.sep).href), true);
    assert.equal(folderLink({ isTTY: false }, file, file, env), file);
    assert.equal(folderLink({ isTTY: true }, file, file, { TERM: 'dumb', ...env }), file);
    assert.equal(folderLink({ isTTY: true }, path.join(root, 'missing'), 'missing', env), 'missing');
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('links require a recognized terminal and labels cannot inject controls', () => {
  for (const env of [{ TERM_PROGRAM: 'iTerm.app' }, { TERM_PROGRAM: 'WezTerm' }, { KITTY_WINDOW_ID: '1' }, { VTE_VERSION: '6000' }]) {
    assert.equal(supportsPathLinks({ isTTY: true }, env), true);
  }
  assert.equal(supportsPathLinks({ isTTY: true }, {}), false);
  assert.equal(folderLink({ isTTY: false }, '.', 'bad\x1b\nlabel'), 'bad  label');
});

test('central output links history, diagnostic paths and truncated labels without changing JSON', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo links (folder) '));
  const previous = process.env.WT_SESSION;
  const previousTerm = process.env.TERM;
  process.env.WT_SESSION = 'test';
  process.env.TERM = 'xterm-256color';
  try {
    const file = path.join(root, 'video with spaces #1.mp4');
    await writeFile(file, 'fixture');
    const env = { WT_SESSION: 'test' };
    for (const line of [`   Saved:  ${file}`, `           ${file}`, `Output: ${root}`, `  ok    Config  ${file} (defaults)`, `Alias installed: name -> veo (${root})`, `Backup: ${file}`]) {
      assert.ok(linkOutputPaths({ isTTY: true }, line, line, env).includes('\x1b]8;;'));
    }
    const original = `Saved: ${file}`;
    const shortened = original.slice(0, 25) + '…';
    const linked = linkOutputPaths({ isTTY: true }, original, shortened, env);
    assert.ok(linked.includes(pathToFileURL(root + path.sep).href));
    assert.ok(linked.includes(shortened.slice(7)));
    assert.equal(linkOutputPaths({ isTTY: true }, `Retry: veo --retry-failed "${file}"`, undefined, env), `Retry: veo --retry-failed "${file}"`);
    await createHistoryRecorder(root)({ url: 'https://example.test/v', title: 'Example', status: 'saved', files: [file] });
    const terminal = { isTTY: true, columns: 60, text: '', write(value) { this.text += value; } };
    await historyMain(['--no-color'], { root, stdout: terminal });
    assert.ok(terminal.text.includes(pathToFileURL(root + path.sep).href));
    terminal.text = '';
    await historyMain(['--json'], { root, stdout: terminal });
    assert.equal(JSON.parse(terminal.text).entries[0].files[0], file);
    assert.ok(!terminal.text.includes('\x1b'));
    const pipe = { isTTY: false, text: '', write(value) { this.text += value; } };
    outputStream(pipe).write(original);
    assert.equal(pipe.text, original);
  } finally {
    if (previous === undefined) delete process.env.WT_SESSION; else process.env.WT_SESSION = previous;
    if (previousTerm === undefined) delete process.env.TERM; else process.env.TERM = previousTerm;
    await rm(root, { recursive: true, force: true });
  }
});
