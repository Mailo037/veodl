import { commandOutput } from './output.js';
import { readFile } from 'node:fs/promises';
import { terminalColumns } from './terminal-size.js';
import { wordWrap } from './progress.js';
import { compareVersions } from './version.js';
import { packageVersion } from './updater.js';
import { suggestOption } from './option-suggestions.js';
import { cleanText } from './utils.js';

/**
 * `veo changes` reads the CHANGELOG.md that ships inside this package, so the
 * notes always belong to the installed code and work without network access. The
 * file is parsed into data instead of being printed: sections, bullets, nested
 * bullets and wrapped lines survive, and `--json` returns the same structure.
 */
const RELEASE = /^##\s+v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)(?:\s+[-–—]\s+(\d{4}-\d{2}-\d{2}))?\s*$/;
const SECTION = /^###\s+(.+?)\s*$/;
const BULLET = /^(\s*)-\s+(.*)$/;
const VERSION = /^\d+(\.\d+)*$/;
const MARKER = '(installed)';
const SHOWN = 5;
const USAGE = 'Usage: veo changes [<version>] [--json] [--latest] [--since <version>] [--to <version>] [--range <from>..<to>]';

export const CHANGES_HELP = `veo changes [<version>] [--json] [--latest] [--since <version>]
                          [--to <version>] [--range <from>..<to>]

List every released version, newest first, and mark the installed one. With a version
(or --version) show that release's notes; a partial version such as 1.10 selects the
newest match. --since shows everything newer than a version, --to and --range set both
ends of a range, and --latest shows only the newest release. Notes come from the
CHANGELOG.md shipped with this veo version, so the command needs no network access and
prints no automatic update hint. Use --json for scripting.
`;

/**
 * Keep a Changelog style document becomes releases, newest first. A heading of
 * second level that is not a version ends the notes, so an appended file list or
 * a second document cannot leak into the last release.
 */
export function parseChangelog(markdown) {
  const releases = [];
  let release = null;
  let section = null;
  let item = null; // Current top-level bullet and parent of any nested ones.
  let last = null; // Most recent bullet, which a wrapped line continues.
  for (const raw of String(markdown).split(/\r?\n/)) {
    const line = raw.replace(/\s+$/, '');
    if (!line.trim()) { last = null; continue; }
    const heading = RELEASE.exec(line);
    if (heading) {
      release = { version: heading[1], date: heading[2] || null, sections: [] };
      releases.push(release);
      section = null; item = null; last = null;
      continue;
    }
    // A second-level heading that is not a version ends the notes; the document
    // title above the first release is preamble and ignored.
    if (/^##\s/.test(line)) break;
    if (!release) continue;
    const type = SECTION.exec(line);
    if (type) {
      section = { type: cleanText(type[1]), items: [] };
      release.sections.push(section);
      item = null; last = null;
      continue;
    }
    if (!section) continue;
    const bullet = BULLET.exec(line);
    if (bullet) {
      const text = cleanText(bullet[2]);
      if (!text) continue;
      if (!bullet[1].length || !item) {
        item = { text, items: [] };
        section.items.push(item);
        last = item;
      } else {
        last = { text, items: [] };
        item.items.push(last);
      }
      continue;
    }
    // Without a bullet the line continues the previous one; the changelog wraps prose.
    if (last) last.text = cleanText(`${last.text} ${line.trim()}`);
  }
  return releases.sort((left, right) => compareVersions(right.version, left.version));
}

// Keep a Changelog order, so every release lists its sections the same way;
// unknown section types follow in the order the changelog lists them.
const SECTION_ORDER = ['Added', 'Changed', 'Deprecated', 'Removed', 'Fixed', 'Security'];
const rank = type => { const index = SECTION_ORDER.indexOf(type); return index < 0 ? SECTION_ORDER.length : index; };
const orderedSections = release => release.sections.filter(section => section.items.length)
  .map((section, index) => ({ section, index }))
  .sort((left, right) => rank(left.section.type) - rank(right.section.type) || left.index - right.index)
  .map(({ section }) => section);

/** Entry counts per section type, in canonical section order. */
export function releaseCounts(release) {
  return Object.fromEntries(orderedSections(release).map(section => [section.type, section.items.length]));
}

const summary = counts => Object.entries(counts).map(([type, count]) => `${type} ${count}`).join(', ');

/** Exact match first, then the newest release starting with the given prefix. */
export function resolveVersion(releases, requested, label = 'version') {
  const wanted = String(requested ?? '').trim().replace(/^v/i, '');
  if (!VERSION.test(wanted)) throw new Error(`${label} must be a version number such as 1.11.0.`);
  const exact = releases.find(release => release.version === wanted);
  if (exact) return exact;
  const partial = releases.filter(release => release.version.startsWith(`${wanted}.`) || release.version.startsWith(`${wanted}-`));
  if (partial.length) return partial.reduce((best, release) => compareVersions(release.version, best.version) > 0 ? release : best);
  const known = releases.map(release => release.version);
  if (!known.length) throw new Error(`${label} "${cleanText(requested)}" cannot be resolved: the changelog lists no releases.`);
  const guess = suggestOption(wanted, known);
  const list = known.slice(0, SHOWN).join(', ');
  throw new Error(`Unknown ${label} "${cleanText(requested)}".${guess ? ` Did you mean "${guess}"?` : ''} Known versions: ${list}${known.length > SHOWN ? ', …' : ''}.`);
}

/** Which releases a request selects, and under which heading they are shown. */
export function selectReleases(releases, { version = null, latest = false, since = null, to = null } = {}) {
  const sorted = [...releases].sort((left, right) => compareVersions(right.version, left.version));
  const newest = sorted[0]?.version ?? null;
  if (version) {
    const release = resolveVersion(sorted, version);
    return { mode: 'version', from: release.version, to: release.version, versions: [release] };
  }
  if (latest) return { mode: 'latest', from: newest, to: newest, versions: sorted.slice(0, 1) };
  if (since || to) {
    const from = since ? resolveVersion(sorted, since, '--since').version : null;
    const upper = to ? resolveVersion(sorted, to, '--to').version : null;
    if (from && upper && compareVersions(upper, from) < 0) throw new Error(`--to ${upper} is older than --since ${from}.`);
    const versions = sorted.filter(release => (!from || compareVersions(release.version, from) > 0)
      && (!upper || compareVersions(release.version, upper) <= 0));
    if (!versions.length) {
      if (from && !upper) throw new Error(`This veo is already at ${from}; no newer changes are listed.`);
      throw new Error(`No changes listed between ${from ?? 'the first release'} and ${upper ?? newest}.`);
    }
    return { mode: 'range', from, to: upper ?? newest, versions };
  }
  return { mode: 'list', from: null, to: null, versions: sorted };
}

/** The shipped changelog, or a clear error when an installation lost the file. */
export async function readChangelog(file = new URL('../CHANGELOG.md', import.meta.url)) {
  let markdown;
  try {
    markdown = await readFile(file, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'EISDIR') throw new Error(`No CHANGELOG.md found at ${file}.`);
    throw error;
  }
  return parseChangelog(markdown);
}

function bulletLines(entry, stream, depth) {
  const prefix = `${'  '.repeat(depth)}- `;
  const hanging = ' '.repeat(prefix.length);
  // Notes are wrapped instead of shortened: a release note that loses its tail
  // because the terminal is narrow would be worse than useless.
  const columns = stream?.isTTY ? Math.max(24, terminalColumns(stream) - prefix.length - 1) : Infinity;
  return wordWrap(entry.text, columns).split('\n').map((line, index) => (index ? hanging : prefix) + line);
}

function releaseLines(release, stream, installed, header) {
  const lines = [];
  if (header) lines.push(release.version + (installed ? `  ${MARKER}` : ''));
  for (const section of orderedSections(release)) {
    lines.push(section.type);
    for (const entry of section.items) {
      lines.push(...bulletLines(entry, stream, 1));
      for (const nested of entry.items) lines.push(...bulletLines(nested, stream, 2));
    }
    lines.push('');
  }
  if (lines.at(-1) === '') lines.pop();
  return lines;
}

const plural = count => `${count} version${count === 1 ? '' : 's'}`;

export function formatChanges(selection, { current, stream } = {}) {
  const { mode, versions } = selection;
  if (!versions.length) return 'veo changes\n\nNo release notes found in CHANGELOG.md.\n';
  if (mode === 'list') {
    const width = Math.max(...versions.map(release => release.version.length));
    const lines = [`veo changes (${plural(versions.length)}, newest first)`, ''];
    for (const release of versions) {
      const installed = release.version === current ? MARKER : '';
      const row = `  ${release.version.padEnd(width)}  ${installed.padEnd(MARKER.length)}`.trimEnd();
      const counts = summary(releaseCounts(release));
      // A release without entries must not leave trailing spaces in the column.
      lines.push(counts ? `${row.padEnd(width + MARKER.length + 4)}  ${counts}` : row);
    }
    lines.push('');
    lines.push('Use veo changes <version> for one version, veo changes --since <version> for');
    lines.push('everything newer than a given version, or --json for scripting.');
    return `${lines.join('\n')}\n`;
  }
  const title = mode === 'range'
    ? `veo changes ${selection.from ? `${selection.from} to ${selection.to}` : `up to ${selection.to}`} (${plural(versions.length)}, newest first)`
    : `veo changes ${selection.from}`;
  const lines = [title, ''];
  for (const release of versions) {
    lines.push(...releaseLines(release, stream, release.version === current, mode === 'range'));
    lines.push('');
  }
  lines.pop();
  return `${lines.join('\n')}\n`;
}

const sectionsJson = release => orderedSections(release)
  .map(section => ({ type: section.type, items: section.items }));

function changesJson(selection, current) {
  if (selection.mode === 'list') {
    return {
      current,
      count: selection.versions.length,
      versions: selection.versions.map(release => {
        const counts = releaseCounts(release);
        return { version: release.version, date: release.date, installed: release.version === current, counts, summary: summary(counts) };
      }),
    };
  }
  if (selection.mode === 'range') {
    return {
      current,
      from: selection.from,
      to: selection.to,
      count: selection.versions.length,
      versions: selection.versions.map(release => ({
        version: release.version, date: release.date, installed: release.version === current, sections: sectionsJson(release),
      })),
    };
  }
  const [release] = selection.versions;
  return { current, version: release.version, date: release.date, installed: release.version === current, sections: sectionsJson(release) };
}

function versionValue(value, flag) {
  if (!value || value.startsWith('-')) throw new Error(`${flag} requires a version number such as 1.11.0.`);
  return value;
}

export function parseChangesArgs(args) {
  const options = { json: false, latest: false, version: null, since: null, to: null };
  for (let index = 0; index < args.length; index++) {
    let arg = args[index];
    let inline = null;
    const equals = arg.indexOf('=');
    if (arg.startsWith('--') && equals > 2) { inline = arg.slice(equals + 1); arg = arg.slice(0, equals); }
    const value = () => versionValue(inline ?? args[++index], arg);
    if (arg === '--json') options.json = true;
    else if (arg === '--latest') options.latest = true;
    else if (arg === '--version') options.version = value();
    else if (arg === '--since') options.since = value();
    else if (arg === '--to') options.to = value();
    else if (arg === '--range') {
      const [from, to] = value().split('..');
      if (!from || !to) throw new Error(`--range must look like 1.9.0..1.11.0. ${USAGE}`);
      options.since ??= from;
      options.to ??= to;
    } else if (arg.startsWith('-')) throw new Error(`Unknown option "${arg}". ${USAGE}`);
    else if (!options.version) options.version = arg;
    else throw new Error(USAGE);
  }
  const ranged = options.since || options.to;
  if (options.version && (ranged || options.latest)) throw new Error(`Use either a version or --latest/--since/--to, not both. ${USAGE}`);
  if (options.latest && ranged) throw new Error(`Use --latest on its own. ${USAGE}`);
  return options;
}

export async function changesMain(args = [], { stdout = process.stdout, current = null, file } = {}) {
  [args, stdout] = commandOutput(args, stdout);
  if (args.some(arg => ['--help', '-h'].includes(arg))) {
    stdout.write(CHANGES_HELP);
    return 0;
  }
  const options = parseChangesArgs(args);
  const installed = current ?? await packageVersion();
  const selection = selectReleases(await readChangelog(file), options);
  if (options.json) stdout.write(`${JSON.stringify(changesJson(selection, installed))}\n`);
  else stdout.write(formatChanges(selection, { current: installed, stream: stdout }));
  return 0;
}
