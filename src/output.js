import { AsyncLocalStorage } from 'node:async_hooks';
import { styleText, terminalText, terminalTitle } from './progress.js';
import { linkOutputPaths } from './path-links.js';
import { terminalColumns } from './terminal-size.js';

const settings = new AsyncLocalStorage();
const HELP_HEADINGS = new Set(['Download:', 'Subtitles and metadata:', 'Playlists and batches:', 'Naming and privacy:', 'Inspect before downloading:', 'Network and performance:', 'Output and scripting:']);
const COUNTER_LABELS = new Set(['Tracking since:', 'Videos saved:', 'Audio saved:', 'Total failures:', 'Skipped:', 'Cancelled:', 'Download time:']);
export const withOutputSettings = (enabled, work) => settings.run({ enabled }, work);

export function outputOptions(args) {
  let profile, color;
  const remaining = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--') { remaining.push(...args.slice(i)); break; }
    if (arg === '--profile' || arg.startsWith('--profile=')) {
      profile = arg === '--profile' ? args[++i] : arg.slice(10);
      if (!profile || profile.startsWith('--')) throw new Error('--profile requires a profile name.');
    } else {
      if (arg === '--color') color = true;
      if (arg === '--no-color') color = false;
      remaining.push(arg);
    }
  }
  return { profile, color, remaining };
}

export function formatOutput(text, stream, enabled = settings.getStore()?.enabled !== false) {
  return text.split(/(\r?\n)/).map(line => {
    if (!line.trim() || /\x1b/.test(line)) return line;
    const value = line.trim();
    const original = line;
    // Limit display values, keeping help, JSON/config text and runnable commands intact.
    if (stream.isTTY) {
      const indent = line.match(/^\s*/)[0];
      const available = { isTTY: true, columns: Math.max(2, terminalColumns(stream) - indent.length) };
      if (/^\d+\. /.test(value)) line = terminalTitle(available, value).split('\n').map(part => indent + part).join('\n');
      else if (/^(?:URL[s]?:|Saved:|Would save:|File:|Missing:|Output:|Job:|Error:|Profile:|Media:|Format:|Streams:|Source:|Title:|Command directory:|Config OK:|veo:|\[?(?:OK|FAIL)\]?\s|[0-9a-z]{6}\s+\d+\s|https?:\/\/)/.test(value)
        || /^\s{6,}\S/.test(line)) line = indent + terminalText(available, value);
    }
    // Report rows such as `  ok    Node.js  v22`: only the status word carries
    // color, so values stay readable; details of passing rows recede.
    const report = /^(\s*)(ok|warn|fail)(\s{2,})(\S.*)$/.exec(line);
    if (report && !/\x1b/.test(original)) {
      const [, indent, level, gap, rest] = report;
      const role = { ok: 'success', warn: 'warn', fail: 'error' }[level];
      const detail = linkOutputPaths(stream, original.slice(original.length - original.trimStart().length + level.length + gap.length), rest);
      return indent + styleText(stream, level, role, enabled) + gap + (level === 'ok' ? styleText(stream, detail, 'muted', enabled) : detail);
    }
    // `Label:   value` rows (veo stats): muted labels, values in the default color.
    const counter = /^([A-Z][A-Za-z ]+:)(\s+)(\S.*)$/.exec(line);
    if (counter && COUNTER_LABELS.has(counter[1])) {
      const [, label, gap, value] = counter;
      const role = label === 'Total failures:' && /^[1-9]/.test(value) ? 'error' : undefined;
      return styleText(stream, label, 'muted', enabled) + gap + (role ? styleText(stream, value, role, enabled) : value);
    }
    let role = 'muted';
    if (/^(?:veo (?:stats|history|changes|changelog|runs|stop|doctor|flush|update|backend|alias|uninstall)\b|veo \d[^ ]* doctor|ID\s+PID\s+STATE|Usage:|Options:|Commands:|Examples:|\d+\. )/.test(value)) role = 'title';
    // A bare version line introduces one release in `veo changes` range output; the
    // list rows carry a summary and stay muted.
    // Indented rows of the version list are never headings, even without a summary.
    if (!/^\s/.test(line) && /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?(?:\s+\(installed\))?$/.test(value)) role = 'title';
    // Group headings of `veo help all`, e.g. "Subtitles and metadata:".
    if (!/^\s/.test(line) && HELP_HEADINGS.has(value)) role = 'title';
    if (/^(?:\[?OK\]?\s|Saved:|Flushed:|Stopped\b|Removed\b|Config OK:|Statistics reset|Managed tools are ready)|\bis up to date\b|\bupdated to\b|\binstalled and will be used\b/i.test(value)) role = 'success';
    if (/^(?:\[?FAIL\]?\s|Error:|veo: (?!note:))|Status:\s*(?:failed|cancelled)/i.test(value)) role = 'error';
    if (/Status:\s*saved/.test(value)) role = 'success';
    if (/^[1-9]\d* problems? found/.test(value)) role = 'error';
    if (/^No problems found/.test(value)) role = /\b[1-9]\d* warnings?\b/.test(value) ? 'warn' : 'success';
    if (/^(?:Playback note:|Warning:)/.test(value)) role = 'warn';
    if (/^Config reset:/.test(value)) role = 'success';
    if (/^Summary:/.test(value)) role = /\b[1-9]\d* failed\b|cancelled/.test(value) ? 'error' : 'success';
    return styleText(stream, linkOutputPaths(stream, original, line), role, enabled);
  }).join('');
}

// Wrap only application-owned prose; no global stream monkey-patching.
export function outputStream(stream, { enabled = settings.getStore()?.enabled !== false, plain = false } = {}) {
  if (plain || !stream.isTTY) return stream;
  return {
    isTTY: stream.isTTY,
    get columns() { return terminalColumns(stream); },
    on(event, listener) { stream.on?.(event, listener); },
    removeListener(event, listener) { stream.removeListener?.(event, listener); },
    write(chunk, ...args) {
      if (typeof chunk !== 'string') return stream.write(chunk, ...args);
      // Live redraw frames already carry erase/cursor sequences; prose fitting
      // and role styling must not touch them, otherwise the invisible styling
      // bytes push raw lines past the terminal width.
      if (chunk.includes('\x1b')) return stream.write(chunk, ...args);
      return stream.write(formatOutput(chunk, stream, enabled), ...args);
    },
  };
}

export function commandOutput(args, ...streams) {
  const enabled = (outputOptions(args).color ?? settings.getStore()?.enabled) !== false;
  const plain = args.includes('--json');
  return [args.filter(arg => arg !== '--no-color' && arg !== '--color'), ...streams.map(stream => outputStream(stream, { enabled, plain }))];
}
