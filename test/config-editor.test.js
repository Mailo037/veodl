import test from 'node:test';
import assert from 'node:assert/strict';
import { PassThrough } from 'node:stream';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { EditorBuffer, colorLine, editConfig, validateEditorText } from '../src/config-editor.js';
import { editorCompletions } from '../src/config-diagnostics.js';
import { CONFIG_GUIDE } from '../src/config-template.js';

test('closing the editor lets the process exit while its stdin remains open', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-exit-'));
  const file = path.join(dir, 'config.json');
  let child, timeout;
  try {
    await writeFile(file, '{}\n');
    const source = `import { editConfig } from './src/config-editor.js';
      process.stdin.isTTY = process.stdout.isTTY = true;
      process.stdin.setRawMode = value => { process.stdin.isRaw = value; };
      await editConfig(process.argv[1]);`;
    child = spawn(process.execPath, ['--input-type=module', '-e', source, file], { stdio: ['pipe', 'pipe', 'pipe'] });
    let opened = false, errors = '';
    child.stderr.on('data', chunk => { errors += chunk; });
    child.stdout.on('data', chunk => {
      if (!opened && chunk.toString().includes('Ctrl+S')) { opened = true; child.stdin.write('\x1b'); }
    });
    timeout = setTimeout(() => child.kill(), 5000);
    const [code, signal] = await once(child, 'exit');
    assert.ok(opened, errors);
    assert.equal(signal, null, 'editor process should exit without being killed');
    assert.equal(code, 0, errors);
  } finally {
    clearTimeout(timeout);
    if (child && child.exitCode === null && child.signalCode === null) child.kill();
    await rm(dir, { recursive: true, force: true });
  }
});

test('semantic errors locate properties and values without matching comments or other profiles', async () => {
  for (const [text, span, message, line] of [
    ['{\n "Quality": "best"\n}', '"Quality"', /Did you mean "quality"/, 2],
    ['{\n "quality": "super"\n}', '"super"', /unsupported value "super".*Choose:.*best/, 2],
    ['{"audio":"ja"}', '"ja"', /boolean.*true, false/, 1],
    ['// "quality": "super"\n{"quality":"best",\n"profiles":{"bad":{"quality":"super"}}}', '"super"', /Profile "bad"/, 3],
    ['\uFEFF{/*comment*/"Qual\\u0069ty":"best"}', '"Qual\\u0069ty"', /Did you mean "quality"/, 1],
    ['{"profiles":{"profiles":{"audio":"yes"}}}', '"yes"', /boolean/, 1],
    ['{"profiles":[]}', '[]', /must be an object/, 1],
    ['{"concurrentFragments":65}', '65', /between 1 and 64/, 1],
    ['{"timeout":"2weeks"}', '"2weeks"', /--timeout expects seconds/, 1],
    ['{"profiles":{"scan":{"timeout":"4s"}}}', '"4s"', /Profile "scan".*between 5 seconds/, 1],
    ['{"activeProfile":"missing","profiles":{"default":{}}}', '"missing"', /Unknown active profile/, 1],
    ['{"profiles":{"list":{}}}', '"list"', /reserved for veo profile list/, 1],
    ['{"profiles":{"reset":{}}}', '"reset"', /reserved for veo profile reset/, 1],
    ['{"quality":"best","quality":"bad"}', '"bad"', /Invalid quality/, 1],
  ]) {
    const issue = await validateEditorText(text);
    assert.ok(issue, text);
    assert.equal(text.slice(issue.start, issue.end), span);
    assert.equal(issue.line, line);
    assert.match(issue.message, message);
  }
  assert.equal(await validateEditorText('{"quality":"900p"}'), null);
  assert.equal(await validateEditorText('{"audio":true,"format":"flac"}'), null);
  const inherited = '{"format":"mp4","profiles":{"music":{"audio":true}}}';
  const issue = await validateEditorText(inherited);
  assert.equal(inherited.slice(issue.start, issue.end), '"mp4"');
  assert.ok(issue.choices.includes('flac'));
  assert.ok(!issue.choices.includes('mp4'));
});

test('completion offers contextual values and spelling corrections without restricting free text', () => {
  const text = '{"audio":true,"profiles":{"music":{"format":"mp3"}}}';
  const options = editorCompletions(text, text.indexOf('mp3'));
  assert.ok(options.choices.includes('flac'));
  assert.ok(!options.choices.includes('mp4'));
  assert.equal(text.slice(options.start, options.end), '"mp3"');
  assert.equal(editorCompletions('{"Quality":"best"}', 3).choices[0], 'quality');
  assert.equal(editorCompletions('{"output":"folder"}', 13), null);
  assert.ok(editorCompletions('{"timeout":"2m"}', 13).choices.includes('2m'));
  assert.deepEqual(editorCompletions('{"activeProfile":"music","profiles":{"default":{},"music":{}}}', 19).choices, ['default', 'music']);
  assert.equal(editorCompletions('{"quality":', 11), null);
});

test('buffer joins and splits lines and clamps vertical movement', () => {
  const buffer = new EditorBuffer('abc\nx');
  buffer.key('end'); buffer.key('return'); buffer.insert('z');
  assert.equal(buffer.text, 'abc\nz\nx');
  buffer.key('home'); buffer.key('backspace');
  assert.equal(buffer.text, 'abcz\nx');
  buffer.key('down'); assert.deepEqual(buffer.position, { row: 1, col: 1 });
  buffer.key('home'); buffer.key('delete'); assert.equal(buffer.text, 'abcz\n');
});

test('validation catches syntax locations, types, unknown keys and CLI constraints', async () => {
  assert.equal(await validateEditorText('// note\n{"audio":true}'), null);
  assert.equal((await validateEditorText('{\n"audio": true,\n}')).line, 3);
  assert.match((await validateEditorText('{"audio":"yes"}')).message, /boolean/);
  assert.match((await validateEditorText('{"typo":true}')).message, /Unknown/);
  assert.ok(await validateEditorText('{"concurrentDownloads":-1}'));
  assert.ok(await validateEditorText('{"profiles":{"bad":{"concurrentDownloads":-1}}}'));
  assert.equal(colorLine('\x1b[2J', false), ' [2J');
});

test('mouse selections support reverse and multiline deletion and replacement', () => {
  const buffer = new EditorBuffer('abc\ndef\nghi');
  buffer.point(2, 2); buffer.point(0, 1, true);
  assert.deepEqual(buffer.selection, [1, 10]);
  buffer.key('backspace'); assert.equal(buffer.text, 'ai');
  buffer.point(0, 0); buffer.point(0, 2, true); buffer.insert('new');
  assert.equal(buffer.text, 'new');
  buffer.point(0, 0); buffer.point(0, 2, true); buffer.key('delete');
  assert.equal(buffer.text, 'w');
  buffer.point(0, 999); assert.equal(buffer.cursor, 1);
});

test('terminal session saves, blocks invalid writes, confirms discard and restores terminal', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-'));
  const file = path.join(dir, 'config.json');
  const input = new PassThrough(), output = new PassThrough();
  input.isTTY = output.isTTY = true;
  input.setRawMode = value => { input.isRaw = value; };
  output.columns = 90; output.rows = 20;
  let screen = ''; output.on('data', chunk => { screen += chunk; });
  const waitFor = async condition => {
    for (let i = 0; i < 200; i++) { if (condition()) return; await new Promise(resolve => setTimeout(resolve, 5)); }
    throw new Error(`Editor did not reach expected state: ${screen.slice(-500).replace(/\x1b\[[\d;?]*[A-Za-z]/g, '')}`);
  };
  const key = (name, text = '', ctrl = false) => input.emit('keypress', text, { name, ctrl });
  try {
    await writeFile(file, '{}\r\n');
    const session = editConfig(file, { input, output });
    await waitFor(() => screen.includes('Ctrl+S'));
    const firstFrame = screen.slice(screen.lastIndexOf('\x1b[H') + 3).split('\r\n');
    assert.equal(firstFrame.length, output.rows, 'the editor fills the terminal without empty status rows');
    assert.match(firstFrame.at(-1), /Ln 1, Col 1/);
    assert.match(firstFrame.at(-1), /Config OK/);
    key('right'); key(undefined, '"audio":true'); key('s', '', true);
    await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":true}\r\n');
    // Real input path: fragmented press, drag and release select true, then replace it.
    screen = '';
    input.write('\x1b[<0;'); input.write('13;2M');
    input.write('\x1b[<32;17;2M\x1b[<0;17;2m');
    assert.ok(screen.includes('\x1b[7mtrue\x1b[0m'));
    input.write('false'); screen = ''; input.write('\x13');
    await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":false}\r\n');
    key('f2'); assert.match(screen, /Options \(1\/2\): true/);
    screen = ''; key('return'); await waitFor(() => screen.includes('Config OK'));
    screen = ''; key('s', '', true);
    await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":true}\r\n');
    key(undefined, ','); key('s', '', true);
    await waitFor(() => screen.includes('Trailing commas'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":true}\r\n');
    key('escape'); assert.match(screen, /Discard unsaved/);
    key('y', 'y'); await session;
    assert.equal(input.isRaw, false);
    assert.equal(input.listenerCount('keypress'), 0);
    assert.equal(input.listenerCount('data'), 0);
    assert.ok(screen.includes('\x1b[?1002l\x1b[?1006l'));
    assert.ok(screen.endsWith('\x1b[?1049l'));
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('wheel scroll keeps cursor in place and Ctrl+C/V exchange the selected text', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-scroll-'));
  const file = path.join(dir, 'config.json');
  const input = new PassThrough(), output = new PassThrough();
  input.isTTY = output.isTTY = true;
  input.setRawMode = value => { input.isRaw = value; };
  output.columns = 100; output.rows = 12;
  let screen = '', copied = '', pasted = false;
  output.on('data', chunk => { screen += chunk; });
  const clipboard = { copy: async text => { copied = text; }, paste: async () => { pasted = true; return copied; } };
  const waitFor = async condition => {
    for (let i = 0; i < 200; i++) { if (condition()) return; await new Promise(resolve => setTimeout(resolve, 5)); }
    throw new Error(`Editor did not reach expected state: ${screen.slice(-500).replace(/\x1b\[[\d;?]*[A-Za-z]/g, '')}`);
  };
  try {
    await writeFile(file, '{\n  "quality": "best"\n' + Array.from({ length: 12 }, (_, i) => `  // line ${i}`).join('\n') + '\n}\n');
    const session = editConfig(file, { input, output, clipboard });
    await waitFor(() => /Ctrl\+A.*All/.test(screen));
    screen = ''; input.write('\x1b[<65;2;3M');
    assert.match(screen, /line 3/);
    assert.match(screen, /Ln 1, Col 1/);
    assert.ok(!screen.includes('\x1b[?25h'), 'cursor is hidden when scrolled offscreen');
    screen = ''; input.write('\x1b[<64;2;3M');
    assert.match(screen, /"quality"/);
    input.write('\x1b[<0;19;3M\x1b[<32;23;3M\x1b[<0;23;3m');
    input.write('\x03');
    await waitFor(() => copied === 'best');
    assert.equal(copied, 'best');
    screen = ''; input.write('\x16');
    await waitFor(() => pasted && screen.includes('Config OK'));
    input.write('\x13');
    await waitFor(() => screen.includes('Saved'));
    assert.match(await readFile(file, 'utf8'), /"quality": "best"/);
    input.write('\x1b'); await session;
    assert.equal(input.isRaw, false);
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('F3 removes an old duplicate guide, can reinsert it, and Ctrl+A selects the file', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-guide-'));
  const file = path.join(dir, 'config.json');
  const settings = '// personal note\n{"quality":"best"}\n';
  const old = '// veo: commented configuration template\n' +
    CONFIG_GUIDE.replace(/(veo: template reference begin) [a-f0-9]{12}/, '$1 older') + settings;
  const input = new PassThrough(), output = new PassThrough();
  input.isTTY = output.isTTY = true;
  input.setRawMode = value => { input.isRaw = value; };
  output.columns = 95; output.rows = 20;
  let screen = '', copied;
  output.on('data', chunk => { screen += chunk; });
  const waitFor = async condition => {
    for (let i = 0; i < 300; i++) { if (condition()) return; await new Promise(resolve => setTimeout(resolve, 5)); }
    throw new Error(`Editor did not reach expected state: ${screen.slice(-400)}`);
  };
  const key = (name, text = '', ctrl = false) => input.emit('keypress', text, { name, ctrl });
  try {
    await writeFile(file, old);
    const session = editConfig(file, { input, output, color: true, clipboard: { copy: async text => { copied = text; }, paste: async () => copied } });
    await waitFor(() => screen.includes('Updated config guide available'));
    assert.match(screen, /\x1b\[1;36mCtrl\+A\x1b\[0m/);
    input.write('\x1bOR'); assert.match(screen.replace(/\x1b\[[\d;]*m/g, ''), /Guide: A add\/update/);
    key('r', 'r'); await waitFor(() => screen.includes('Generated guide removed'));
    assert.equal(await readFile(file, 'utf8'), old, 'no change before saving');
    screen = ''; key('s', '', true); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), settings);
    key('f3'); key('a', 'a'); await waitFor(() => screen.includes('Guide added/updated'));
    screen = ''; key('s', '', true); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), CONFIG_GUIDE + settings);
    input.write('\x01');
    assert.match(screen, /All text selected/);
    key('c', '', true); await waitFor(() => copied !== undefined);
    assert.equal(copied, CONFIG_GUIDE + settings);
    key('escape'); await session;
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('Ctrl+Z and Ctrl+Y undo and redo grouped typing, replacement and paste', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-history-'));
  const file = path.join(dir, 'config.json');
  const input = new PassThrough(), output = new PassThrough();
  input.isTTY = output.isTTY = true;
  input.setRawMode = value => { input.isRaw = value; };
  output.columns = 95; output.rows = 15;
  let screen = '';
  let pasteText = 'true';
  output.on('data', chunk => { screen += chunk; });
  const waitFor = async condition => {
    for (let i = 0; i < 200; i++) { if (condition()) return; await new Promise(resolve => setTimeout(resolve, 5)); }
    throw new Error('Editor did not reach expected state');
  };
  try {
    await writeFile(file, '{}\n');
    const session = editConfig(file, { input, output, clipboard: { copy: async () => {}, paste: async () => pasteText } });
    await waitFor(() => /Ctrl\+Z.*Undo/.test(screen));
    input.write('\x1b[C');
    input.write('"audio":');
    screen = ''; input.write('\x16'); await waitFor(() => screen.includes('Config OK'));
    screen = ''; input.write('\x1a'); await waitFor(() => screen.includes('Undone.'));
    assert.equal(await readFile(file, 'utf8'), '{}\n', 'undo does not write an invalid intermediate state');
    screen = ''; input.write('\x19'); await waitFor(() => screen.includes('Redone.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":true}\n');
    screen = ''; input.write('\x1a'); await waitFor(() => screen.includes('Undone.'));
    screen = ''; input.write('\x1a'); await waitFor(() => screen.includes('Undone.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{}\n', 'typed characters undo as one edit');
    screen = ''; input.write('\x19'); await waitFor(() => screen.includes('Redone.'));
    screen = ''; input.write('\x19'); await waitFor(() => screen.includes('Redone.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":true}\n');
    screen = ''; input.write('\x1a'); await waitFor(() => screen.includes('Undone.'));
    input.write('false');
    input.write('\x19'); await waitFor(() => screen.includes('Nothing to redo.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":false}\n', 'a new edit clears the redo history');
    pasteText = '{"quality":"best"}';
    input.write('\x01');
    screen = ''; input.write('\x16'); await waitFor(() => screen.includes('Config OK'));
    screen = ''; input.write('\x1a'); await waitFor(() => screen.includes('Undone.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), '{"audio":false}\n', 'undo restores text replaced by paste');
    screen = ''; input.write('\x19'); await waitFor(() => screen.includes('Redone.'));
    input.write('\x13'); await waitFor(() => screen.includes('Saved'));
    assert.equal(await readFile(file, 'utf8'), pasteText);
    input.write('\x1b'); await session;
    assert.equal(input.isRaw, false);
    assert.equal(input.isPaused(), true, 'editor releases input that was not flowing before it opened');
  } finally { await rm(dir, { recursive: true, force: true }); }
});

test('dragging at viewport edges scrolls and extends selection until release', async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'veo-editor-drag-scroll-'));
  const file = path.join(dir, 'config.json');
  const input = new PassThrough(), output = new PassThrough();
  input.isTTY = output.isTTY = true;
  input.setRawMode = value => { input.isRaw = value; };
  output.columns = 85; output.rows = 12;
  let screen = '', copied = '';
  output.on('data', chunk => { screen += chunk; });
  const waitFor = async condition => {
    for (let i = 0; i < 200; i++) { if (condition()) return; await new Promise(resolve => setTimeout(resolve, 10)); }
    throw new Error(`Editor did not reach expected state: copied=${JSON.stringify(copied)} screen=${screen.slice(-400)}`);
  };
  try {
    const lines = Array.from({ length: 25 }, (_, i) => `// line ${i}`);
    await writeFile(file, lines.join('\n') + '\n{}\n');
    const session = editConfig(file, { input, output, clipboard: { copy: async value => { copied = value; }, paste: async () => '' } });
    await waitFor(() => /Ctrl\+Z.*Undo/.test(screen));
    screen = '';
    input.write('\x1b[<0;5;3M\x1b[<32;5;10M');
    await waitFor(() => /line 11/.test(screen));
    input.write('\x1b[<0;5;10m\x03');
    await waitFor(() => copied.includes('line 10'));
    assert.match(copied, /line 1[\s\S]*line 10/);
    screen = '';
    await new Promise(resolve => setTimeout(resolve, 220));
    assert.equal(screen, '', 'released drag does not keep scrolling');

    input.write('\x1b[<65;2;3M\x1b[<65;2;3M\x1b[<65;2;3M');
    screen = '';
    input.write('\x1b[<0;5;8M\x1b[<32;5;2M');
    await waitFor(() => /line 0/.test(screen));
    input.write('\x1b[<0;5;2m');
    copied = '';
    input.write('\x03'); await waitFor(() => copied !== '');
    assert.match(copied, /line 0[\s\S]*line/);
    input.write('\x1b'); await session;
  } finally { await rm(dir, { recursive: true, force: true }); }
});
