import { cleanText } from './utils.js';
import { createTerminalTitle } from './terminal-title.js';
import { terminalColumns } from './terminal-size.js';
import { createInlineRegion } from './inline-region.js';

const WARNING_STATUS = /^(?:Playback note:|Playback compatibility could not|Warning:)/;
const PROCESSING_LABELS = { Merger: 'Merging audio/video', VideoRemuxer: 'Changing video container', VideoConvertor: 'Converting video', ExtractAudio: 'Converting audio', EmbedSubtitle: 'Embedding subtitles', Metadata: 'Writing metadata', EmbedThumbnail: 'Embedding thumbnail', MoveFiles: 'Preparing saved file' };

export function styleText(stream, text, role = 'muted', enabled = true, env = process.env) {
  if (!enabled || !stream.isTTY || Object.hasOwn(env, 'NO_COLOR') || env.TERM === 'dumb') return text;
  const codes = { muted: 90, title: 1, profile: 97, success: 32, warn: 33, error: 31, accent: 36 };
  return '\x1b[' + (codes[role] || 90) + 'm' + text + '\x1b[0m';
}

function bytes(value) {
  if (!Number.isFinite(value) || value < 0) return '?';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let i = 0;
  while (value >= 1024 && i < units.length - 1) { value /= 1024; i++; }
  return `${value.toFixed(i ? 1 : 0)} ${units[i]}`;
}

// Count terminal cells conservatively, including wide titles and emoji.
function cells(text) {
  return [...text].reduce((width, char) => width + (/\p{Mark}|\u200d/u.test(char) ? 0 : /[\u1100-\u115f\u2329\u232a\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\ufe10-\ufe19\ufe30-\ufe6f\uff01-\uff60\uffe0-\uffe6]|\p{Extended_Pictographic}/u.test(char) ? 2 : 1), 0);
}

function fit(text, width) {
  if (cells(text) <= width) return text;
  let result = '';
  for (const char of text) {
    if (cells(result + char) > width - 1) break;
    result += char;
  }
  return width > 0 ? result + '…' : '';
}

export function wrap(text, width) {
  if (!Number.isFinite(width)) return text;
  const lines = [];
  let line = '';
  for (const char of text) {
    if (line && cells(line + char) > width) { lines.push(line); line = ''; }
    line += char;
  }
  return [...lines, line].join('\n');
}

// Break at spaces; only a word wider than the line is split.
export function wordWrap(text, width) {
  if (!Number.isFinite(width)) return text;
  const lines = [];
  let line = '';
  for (const word of text.split(/ +/)) {
    const candidate = line ? `${line} ${word}` : word;
    if (cells(candidate) <= width) { line = candidate; continue; }
    if (line) lines.push(line);
    line = '';
    const pieces = wrap(word, width).split('\n');
    line = pieces.pop();
    lines.push(...pieces);
  }
  return [...lines, line].join('\n');
}

/**
 * A labelled path wrapped over several rows instead of shortened: a saved file
 * name that loses its tail is the one detail a user cannot recover. Rows break
 * after a path separator when one fits, and continue under the path's first
 * column. Returns the path pieces; rows are `label + pieces[0]`, then indent.
 */
export function pathRows(stream, label, value) {
  const safe = cleanText(value);
  const width = stream.isTTY ? Math.max(1, terminalColumns(stream) - 1 - cells(label)) : Infinity;
  if (width < 8 || cells(safe) <= width) return [safe];
  const rows = [];
  let rest = safe;
  while (cells(rest) > width) {
    let piece = '';
    for (const char of rest) {
      if (cells(piece + char) > width) break;
      piece += char;
    }
    const separator = Math.max(piece.lastIndexOf('/'), piece.lastIndexOf('\\'));
    if (separator >= width / 3) piece = piece.slice(0, separator + 1);
    rows.push(piece);
    rest = rest.slice(piece.length);
  }
  if (rest) rows.push(rest);
  return rows;
}

// Block glyphs need a terminal that understands Unicode; C/POSIX locales and
// dumb terminals keep the ASCII bar.
export function supportsUnicode(stream, env = process.env) {
  if (!stream.isTTY || env.TERM === 'dumb') return false;
  const locale = env.LC_ALL || env.LC_CTYPE || env.LANG || '';
  return !/^(?:C|POSIX)$/i.test(locale);
}

export function terminalText(stream, text) {
  return fit(cleanText(text), stream.isTTY ? Math.max(1, terminalColumns(stream) - 1) : Infinity);
}

export function terminalTitle(stream, text) {
  const safe = cleanText(text);
  if (!stream.isTTY) return safe;
  const width = Math.max(1, terminalColumns(stream) - 1);
  const lines = [];
  let line = '';
  for (const char of safe) {
    if (cells(line + char) > width) {
      if (lines.length === 2) return [...lines, fit(line + char + '…', width)].join('\n');
      lines.push(line);
      line = '';
    }
    line += char;
  }
  return [...lines, line].join('\n');
}

const BAR_WIDTH = 20;

// Fixed-width fields keep the bar and statistics from jumping between updates.
function progressBar(percent, size, { unicode, paint, frame }) {
  const [full, empty] = unicode ? ['█', '░'] : ['=', '-'];
  const wrapBar = text => unicode ? text : `[${text}]`;
  if (percent === null) {
    // Unknown total: a short block travels back and forth instead of an empty bar.
    const block = Math.max(1, Math.min(3, size - 1));
    const span = Math.max(1, size - block);
    const step = frame % (span * 2);
    const start = step < span ? step : span * 2 - step;
    return wrapBar(paint(empty.repeat(start), 'muted') + paint(full.repeat(block), 'accent') + paint(empty.repeat(size - block - start), 'muted'));
  }
  const filled = Math.round(percent / 100 * size);
  return wrapBar(paint(full.repeat(filled), 'accent') + paint(empty.repeat(size - filled), 'muted'));
}

/**
 * Renders one progress update. `unicode` selects block glyphs and `paint(text,
 * role)` colors the bar; both default to the plain ASCII form used in logs.
 */
export function formatProgress(data, { columns = Infinity, prefix = '', unicode = false, paint = text => text, frame = Math.floor(Date.now() / 120) } = {}) {
  const done = Number.isFinite(data.downloaded_bytes) ? data.downloaded_bytes : 0;
  const total = data.total_bytes || data.total_bytes_estimate;
  const estimated = !data.total_bytes && Boolean(data.total_bytes_estimate);
  const percent = total > 0 ? Math.max(0, Math.min(estimated && data.status !== 'finished' ? 99 : 100, done / total * 100)) : null;
  const eta = Number.isFinite(data.eta) ? `${Math.floor(data.eta / 60)}:${String(Math.floor(data.eta % 60)).padStart(2, '0')}` : '?';
  const pct = `${estimated && data.status !== 'finished' ? '~' : ''}${percent === null ? '?' : percent.toFixed(0)}%`.padStart(4);
  const size = total > 0 ? `${bytes(done)} / ${estimated ? '~' : ''}${bytes(total)}` : `${bytes(done)} received`;
  const label = prefix ? cleanText(prefix) + ' ' : '';
  if (data.status === 'finished') return wrap(`${label}${bytes(done)} received; processing…`, columns);
  const stats = [`${bytes(data.speed)}/s`.padStart(11), size.padStart(19), `ETA ${eta}`.padEnd(9)];
  const decoration = unicode ? 1 : 3;
  const barSize = Math.max(1, Math.min(BAR_WIDTH, Number.isFinite(columns) ? columns - cells(pct) - decoration : BAR_WIDTH));
  const plainBar = `${progressBar(percent, barSize, { unicode, paint: text => text, frame })} ${pct}`;
  const bar = `${progressBar(percent, barSize, { unicode, paint, frame })} ${pct}`;
  const full = `${label}${plainBar} ${stats.join('  ')}`;
  if (cells(full.trimEnd()) <= columns) return `${label}${bar} ${stats.join('  ')}`.trimEnd();
  // Too narrow for aligned fields: keep one line without padding if possible.
  const compact = stats.map(value => value.trim()).join('  ');
  if (cells(`${label}${plainBar}  ${compact}`) <= columns) return `${label}${bar}  ${compact}`;
  const lines = [];
  if (label) lines.push(fit(label.trimEnd(), columns));
  lines.push(cells(plainBar) <= columns ? bar : wrap(plainBar, columns));
  let statLine = '';
  for (const stat of stats.map(value => value.trim())) {
    if (statLine && cells(`${statLine}  ${stat}`) > columns) { lines.push(wrap(statLine, columns)); statLine = ''; }
    statLine += (statLine ? '  ' : '') + stat;
  }
  if (statLine) lines.push(wrap(statLine, columns));
  return lines.join('\n');
}

export function createReporter(stream = process.stderr, { setTitle = createTerminalTitle(stream), env = process.env } = {}) {
  let color = true;
  let region;
  const muted = text => styleText(stream, text, 'muted', color, env);
  const heading = text => styleText(stream, text, 'title', color, env);
  let active = false;
  let lastLog = 0;
  let name = '';
  let phase = 'Starting…';
  let started = false;
  let position = '';
  let hasItem = false;
  let streamName = '';
  let animation;
  let animationLabel = '';
  let animationOwner;
  let animationFrame = 0;
  let redraw;
  let listening = false;
  let sizeTimer;
  let lastWidth = terminalColumns(stream);
  const append = render => {
    if (region) region.log(render); else stream.write(render());
  };
  const eraseLive = () => {
    if (!active) return;
    stream.write('\r\x1b[2K\x1b[J');
  };
  const renderLive = text => {
    eraseLive();
    const rows = text.split('\n').length;
    // Park the real cursor at the block's first row. The terminal relocates it
    // during reflow; no saved cursor or guessed old line count is needed on resize.
    stream.write(`\r\x1b[2K${text.replace(/\n/g, '\r\n')}${rows > 1 ? `\x1b[${rows - 1}A` : ''}\r`);
    active = true;
  };
  const onResize = () => {
    const width = terminalColumns(stream);
    if (width === lastWidth) return;
    lastWidth = width;
    // Terminal scrollback reflows differently across hosts. Never infer cursor
    // positions for past log/title lines; redraw only the owned live block.
    if (active) redraw?.();
  };
  const watchResize = render => {
    if (region) return;
    if (stream.isTTY && terminalColumns(stream) !== lastWidth) onResize();
    redraw = render;
    if (!sizeTimer && stream.isTTY) {
      lastWidth = terminalColumns(stream);
      sizeTimer = setInterval(() => {
        const width = terminalColumns(stream);
        if (width !== lastWidth) onResize();
      }, 150);
      sizeTimer.unref?.();
    }
    if (!listening && stream.isTTY && stream.on) {
      stream.on('resize', onResize);
      listening = true;
    }
  };
  const stopAnimation = () => { if (animation) clearInterval(animation); animation = undefined; };
  const display = value => terminalText(stream, value);
  // Status notes wrap at word boundaries: a hint cut off mid-option is useless.
  // Like titles, at most three rows: a long hint stays whole, a pasted token does not flood the log.
  const block = value => {
    const width = stream.isTTY ? Math.max(1, terminalColumns(stream) - 1) : Infinity;
    const rows = wordWrap(cleanText(value), width).split('\n');
    if (rows.length <= 3) return rows;
    return [...rows.slice(0, 2), fit(rows.slice(2).join(' '), width)];
  };
  const statusBlock = value => {
    const role = WARNING_STATUS.test(cleanText(value).replace(/^(?:\[\d+\/\d+\] )+[^:]*: /, '')) ? 'warn' : 'muted';
    return block(value).map(row => styleText(stream, row, role, color, env)).join('\n') + '\n';
  };
  const animate = (label, owner) => {
    clear();
    if (!stream.isTTY) { stream.write(muted(label + '…') + '\n'); return; }
    animationLabel = label;
    animationOwner = owner;
    animationFrame = 0;
    const tick = () => {
      const render = () => muted(display(animationLabel + '.'.repeat(animationFrame % 3 + 1)));
      if (region) region.live(render); else renderLive(render());
      animationFrame++;
    };
    watchResize(tick);
    tick();
    animation = setInterval(tick, 350);
    animation.unref?.();
  };
  const processingResult = (label, status) => {
    clear();
    const failed = status === 'failed' || status === 'error';
    const suffix = failed ? 'failed' : 'done';
    append(() => muted(`${fit(label, Math.max(0, (stream.isTTY ? terminalColumns(stream) - 1 : Infinity) - suffix.length - 2))}: `) + styleText(stream, suffix, failed ? 'error' : 'success', color, env) + '\n');
    watchResize(undefined);
  };
  const finishStatus = (message, owner, prefix = '', outcome) => {
    const label = prefix + cleanText(message);
    if (!animation || animationOwner !== owner || animationLabel !== label.slice(0, -1)) return;
    if (outcome) { processingResult(label.slice(0, -1), outcome); return; }
    clear();
    append(() => muted(display(label)) + '\n');
    watchResize(undefined);
  };
  const paint = (text, role) => text ? styleText(stream, text, role, color, env) : text;
  const line = (data, prefix) => formatProgress(data, { prefix, columns: stream.isTTY ? Math.max(1, terminalColumns(stream) - 1) : Infinity, unicode: supportsUnicode(stream, env), paint });
  const draw = (data, prefix) => {
    if (region) { region.live(() => line(data, prefix)); return; }
    const render = () => renderLive(line(data, prefix));
    watchResize(render);
    render();
  };
  const updateTitle = () => setTitle(`veo | ${phase}${name ? ` | ${name}` : ''}`);
  function clear() {
    stopAnimation();
    region?.live(undefined);
    if (sizeTimer) clearInterval(sizeTimer);
    sizeTimer = undefined;
    if (listening) stream.removeListener?.('resize', onResize);
    listening = false;
    redraw = undefined;
    if (active && stream.isTTY) { eraseLive(); stream.write('\r\x1b[2K'); }
    active = false;
    animationLabel = '';
    animationOwner = undefined;
  }
  const scoped = (index, total, title, parent = '') => {
    let childName = cleanText(title), lastProgress = 0;
    const owner = Symbol('scoped reporter');
    const prefix = parent + '[' + index + '/' + total + '] ';
    const log = (message, quiet = true) => {
      phase = cleanText(message); name = childName;
      const line = prefix + childName + ': ' + phase;
      if (phase.endsWith('…')) animate(line.slice(0, -1), owner);
      else { clear(); append(() => (quiet ? muted(display(line)) : display(line)) + '\n'); }
      if (started) updateTitle();
    };
    return {
      scoped: (index, total, title) => scoped(index, total, title, prefix),
      name(value) { childName = cleanText(value); },
      status: log,
      finishStatus(message, outcome) { finishStatus(message, owner, prefix + childName + ': ', outcome); },
      progress(data) {
        if (stream.isTTY) {
          draw(data, prefix + childName + ': ' + (data.stream || 'Media'));
        } else if (Date.now() - lastProgress >= 5000 || data.status === 'finished') {
          log((data.stream || 'Media') + ' ' + formatProgress(data), false);
          lastProgress = Date.now();
        }
      },
      processing(data) {
        const label = prefix + childName + ': ' + (PROCESSING_LABELS[data.postprocessor] || 'Processing media');
        if (['finished', 'failed', 'error'].includes(data.status)) processingResult(label, data.status);
        else animate(label, owner);
        phase = `${PROCESSING_LABELS[data.postprocessor] || 'Processing media'}${data.status === 'finished' ? ': done' : '…'}`;
        name = childName; if (started) updateTitle();
      },
      finish() {},
      failStep() { if (animation && animationOwner === owner) processingResult(animationLabel, 'failed'); },
    };
  };
  return {
    enableInline() {
      if (!region && stream.isTTY && env.TERM !== 'dumb') { clear(); region = createInlineRegion(stream); }
    },
    output(destination) {
      if (!stream.isTTY || !destination.isTTY) return destination;
      return {
        isTTY: true,
        get columns() { return terminalColumns(destination); },
        write(chunk, ...args) {
          clear();
          region?.commit();
          return destination.write(chunk, ...args);
        },
      };
    },
    dispose() { clear(); region?.close(); region = undefined; },
    configure(options) { color = options.color !== false; },
    scoped,
    item(index, total, title) {
      clear(); position = total > 1 ? `[${index}/${total}] ` : ''; hasItem = true; name = cleanText(title); streamName = ''; lastLog = 0;
      const titleText = `${position}${name}`;
      // The URL only identifies the request; the media title that follows is the heading.
      const style = /^https?:\/\//i.test(name) ? muted : heading;
      const render = () => style(terminalTitle(stream, titleText)) + '\n';
      if (region) region.source(render); else append(render);
      watchResize(undefined);
      phase = 'Starting…'; if (started) updateTitle();
    },
    processing(data) {
      const processor = cleanText(data.postprocessor || 'Processing');
      const label = PROCESSING_LABELS[processor] || 'Processing media';
      phase = `${label}${data.status === 'finished' ? ': done' : '…'}`;
      if (started) updateTitle();
      if (['finished', 'failed', 'error'].includes(data.status)) processingResult(`${position}${label}`, data.status);
      else animate(`${position}${label}`);
    },
    start(title = '') { started = true; name = cleanText(title); phase = 'Starting…'; updateTitle(); },
    name(title) {
      const next = cleanText(title);
      if (hasItem && next !== name) {
        clear();
        const titleText = `${position}${next}`;
        const render = () => heading(terminalTitle(stream, titleText)) + '\n';
        // Titles join the log stream at their chronological position, like
        // without the inline region. Stored as a render function, they still
        // re-render on resize (max three rows, current width).
        if (region) region.log(render); else append(render);
        watchResize(undefined);
      }
      name = next; if (started) updateTitle();
    },
    status(message) {
      phase = cleanText(message);
      if (started) updateTitle();
      if (phase.endsWith('…')) animate(phase.slice(0, -1));
      else {
        clear();
        const status = phase;
        append(() => statusBlock(status));
        watchResize(undefined);
      }
    },
    profile(value) {
      const selected = value ? cleanText(value) : 'global (no profile)';
      phase = `Profile: ${selected}`;
      if (started) updateTitle();
      clear();
      append(() => value && value !== 'default'
        ? muted('Profile: ') + styleText(stream, fit(selected, stream.isTTY ? Math.max(0, terminalColumns(stream) - 10) : Infinity), 'profile', color, env) + '\n'
        : muted(display(`Profile: ${selected}`)) + '\n');
    },
    finishStatus(message, outcome) { finishStatus(message, undefined, '', outcome); },
    failStep() { if (animation && animationOwner === undefined) processingResult(animationLabel, 'failed'); },
    complete() { clear(); phase = 'Done'; if (started) updateTitle(); },
    fail(cancelled = false) { clear(); phase = cancelled ? 'Cancelled' : 'Failed'; if (started) updateTitle(); },
    progress(data) {
      if (data.stream && data.stream !== streamName) {
        clear(); streamName = data.stream; lastLog = 0;
        const label = `${position}${streamName} download`;
        append(() => muted(display(label)) + '\n');
      }
      const total = data.total_bytes;
      const percent = Number.isFinite(total) && total > 0 && Number.isFinite(data.downloaded_bytes)
        ? Math.max(0, Math.min(100, Math.round(data.downloaded_bytes / total * 100))) : null;
      phase = data.status === 'finished' ? 'Processing…' : percent === null ? 'Downloading…' : `${percent}%`;
      if (started) updateTitle();
      if (stream.isTTY) {
        draw(data, `${position}${streamName}`);
      } else if (Date.now() - lastLog >= 5000 || data.status === 'finished') {
        stream.write(`${position}${streamName ? `${streamName} ` : ''}${formatProgress(data)}\n`);
        lastLog = Date.now();
      }
    },
    finish() { clear(); region?.commit(); },
  };
}
