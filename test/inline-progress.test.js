import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import xterm from '@xterm/headless';
import { createReporter, terminalTitle } from '../src/progress.js';

test('inline progress preserves shell scrollback, never enters fullscreen and keeps result output', async () => {
  const term = new xterm.Terminal({ cols: 100, rows: 24, allowProposedApi: true, convertEol: true });
  let pending = Promise.resolve();
  let raw = '';
  const stream = Object.assign(new EventEmitter(), { isTTY: true, columns: 100, rows: 24,
    write(text) { raw += text; pending = pending.then(() => new Promise(resolve => term.write(text, resolve))); return true; },
  });
  const reporter = createReporter(stream, { setTitle() {}, env: {} });
  reporter.configure({ color: false });
  stream.write('Shell history remains here\n');
  reporter.enableInline();
  const url = 'https://example.test/' + 'signed-url'.repeat(150);
  reporter.item(1, 1, url);
  const progress = { stream: 'Media', downloaded_bytes: 90 * 1024 ** 2,
    total_bytes_estimate: 1024 ** 3, speed: 12.6 * 1024 ** 2, eta: 77 };
  reporter.progress(progress);
  const text = () => Array.from({ length: term.buffer.active.length }, (_, i) => term.buffer.active.getLine(i)?.translateToString(true) || '').join('\n');
  try {
    for (const columns of [40, 120, 30, 90, 50, 160, 40]) {
      await pending;
      term.resize(columns, 24); stream.columns = columns; stream.emit('resize');
      await pending;
      assert.equal(term.buffer.active.type, 'normal');
      const visible = text();
      assert.equal(visible.split('Shell history remains here').length - 1, 1, visible);
      assert.equal(visible.split('https://example.test/').length - 1, 1, visible);
      const titleStart = visible.indexOf('https://example.test/');
      assert.equal(visible.slice(titleStart).split('\n').slice(0, 3).join('\n'), terminalTitle(stream, url));
      assert.equal(visible.split('ETA 1:17').length - 1, 1, visible);
      for (const value of ['12.6 MiB/s', '90.0 MiB / ~1.0 GiB']) assert.ok(visible.includes(value), visible);
    }
    reporter.name('Readable title');
    await pending;
    assert.match(text(), /https:\/\/example.test\//);
    assert.match(text(), /Readable title/);
    reporter.output(stream).write('Saved: example.mp4\n');
    reporter.complete();
  } finally { reporter.dispose(); await pending; }
  assert.match(text(), /Saved: example.mp4/);
  assert.doesNotMatch(raw, /\x1b\[\?1049|\x1b\[2J|\x1b\[H/);
  assert.equal(stream.listenerCount('resize'), 0);
  term.dispose();
});
