import { terminalColumns } from './terminal-size.js';
import { stripVTControlCharacters } from 'node:util';

/** A bounded region in the normal terminal buffer, with the cursor at its end. */
export function createInlineRegion(stream) {
  let source;
  let live;
  let visible = false;
  let painting = false;
  let closed = false;
  let lastSize = '';
  let rendered = [];
  const logs = [];
  const erase = () => {
    if (!visible) return;
    const width = terminalColumns(stream);
    const lastReflows = process.platform === 'win32' && (stream === process.stdout || stream === process.stderr);
    const above = rendered.reduce((sum, line, index) => sum + (index === rendered.length - 1 && !lastReflows ? 1 : Math.max(1, Math.ceil(stripVTControlCharacters(line).length / width))), 0) - 1;
    if (above) stream.write(`\x1b[${above}A`);
    stream.write('\r\x1b[2K\x1b[J');
  };
  const textOf = render => render().replace(/\r?\n$/, '').replace(/\r\n/g, '\n');
  const render = () => {
    if (closed || painting) return;
    painting = true;
    try {
      const width = terminalColumns(stream);
      const height = Math.max(1, (stream.rows || 24) - 1);
      lastSize = `${width}:${height}`;
      erase();
      const sourceRows = source ? textOf(source).split('\n') : [];
      const progress = live ? textOf(live).split('\n') : [];
      const logLines = () => logs.flatMap(log => textOf(log).split('\n'));
      // Commit older status messages above the region before it outgrows the
      // viewport. Source, logs and progress stay in chronological order and
      // re-render together when the terminal is resized.
      while (logs.length && sourceRows.length + logLines().length + progress.length > height) {
        stream.write(textOf(logs.shift()).replace(/\n/g, '\r\n') + '\r\n');
      }
      const contents = [...sourceRows, ...logLines(), ...progress];
      if (!contents.length) { visible = false; return; }
      const lines = contents;
      stream.write('\r' + lines.join('\r\n'));
      rendered = lines;
      visible = true;
    } finally { painting = false; }
  };
  const resize = () => {
    const size = `${terminalColumns(stream)}:${Math.max(1, (stream.rows || 24) - 1)}`;
    if (size !== lastSize) render();
  };
  stream.on?.('resize', resize);
  const timer = setInterval(resize, 150);
  timer.unref?.();
  const commit = () => {
    erase();
    const lines = [...[source].filter(Boolean).map(textOf), ...logs.map(textOf)];
    if (lines.length) stream.write(lines.join('\n').replace(/\n/g, '\r\n') + '\r\n');
    source = undefined; logs.length = 0; live = undefined; visible = false;
  };
  return {
    source(value) { commit(); source = value; render(); },
    log(value) { logs.push(value); render(); },
    live(value) { live = value; render(); },
    commit,
    close() {
      if (closed) return;
      commit(); closed = true;
      clearInterval(timer); stream.removeListener?.('resize', resize);
    },
  };
}
