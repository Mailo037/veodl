// Usage: node scripts/smoke-terminal.js <directory containing node_modules/node-pty>
// node-pty is a temporary validation tool, not a runtime dependency of veo.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import xterm from '@xterm/headless';
const require = createRequire(path.resolve(process.argv[2] || '.', 'package.json'));
const pty = require('node-pty');
const root = fileURLToPath(new URL('../', import.meta.url));
const term = new xterm.Terminal({ cols: 100, rows: 24, allowProposedApi: true });
const child = pty.spawn(process.execPath, [path.join(root, 'scripts/terminal-fixture.js')], {
  name: 'xterm-256color', cols: 100, rows: 24, cwd: root, env: { ...process.env, TERM: 'xterm-256color' },
});
let pending = Promise.resolve();
child.onData(text => { pending = pending.then(() => new Promise(resolve => term.write(text, resolve))); });
term.onData(text => child.write(text));
let didExit = false;
const exited = new Promise(resolve => child.onExit(result => { didExit = true; resolve(result); }));
const lines = () => Array.from({ length: term.rows }, (_, i) => term.buffer.active.getLine(term.buffer.active.viewportY + i)?.translateToString(true) || '');
try {
  await delay(800); await pending;
  for (const [cols, rows] of [[40, 18], [120, 24], [30, 12], [90, 20], [50, 16], [160, 24], [40, 18]]) {
    term.resize(cols, rows); child.resize(cols, rows);
    await delay(600); await pending;
    const screen = lines().join('\n');
    const history = Array.from({ length: term.buffer.active.length }, (_, i) => term.buffer.active.getLine(i)?.translateToString(true) || '').join('\n');
    // Shrinking can push a previous rendering into terminal scrollback before
    // veo receives the resize event. Validate the current viewport, not history.
    assert.equal(screen.split('https://example.test/').length - 1, 1, screen);
    const urlLines = screen.slice(screen.indexOf('https://example.test/')).split('\n');
    assert.ok(urlLines[2].trimEnd().endsWith('…'), screen);
    assert.equal(urlLines[3].trimEnd(), 'Reading video: done', screen);
    assert.equal(history.split('Reading video: done').length - 1, 1, history);
    assert.match(screen, /\[[=-]+\]|[█░]{20}/);
    for (const value of ['12.6 MiB/s', '90.0 MiB / ~1.0 GiB', 'ETA 1:17']) assert.ok(screen.includes(value), screen);
    assert.equal(term.buffer.active.type, 'normal');
    process.stdout.write(`PASS ${cols}x${rows}: URL limited to 3 rows, normal flow, bar, speed, size, ETA\n`);
  }
  child.write('n');
  await delay(300); await pending;
  assert.match(lines().join('\n'), /Resize title/);
  assert.match(lines().join('\n'), /https:\/\/example.test\//);
  child.write('q');
  const result = await Promise.race([exited, delay(5000).then(() => { throw new Error('Fixture did not exit'); })]);
  await pending;
  assert.equal(result.exitCode, 0);
  assert.match(lines().join('\n'), /TERMINAL_RESTORED/);
  assert.match(lines().join('\n'), /Saved: fixture.mp4/);
  process.stdout.write('PASS exit: terminal restored, saved result retained\n');
} finally { if (!didExit) child.kill(); term.dispose(); }
// node-pty's Windows agent can retain its input pipe after a confirmed child exit.
// All assertions and output have completed; do not leave the smoke host running.
await new Promise(resolve => process.stdout.write('', resolve));
process.exit(0);
