import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import {
  CHANGES_HELP, changesMain, parseChangesArgs, parseChangelog, readChangelog, releaseCounts, resolveVersion, selectReleases,
} from '../src/changes.js';

const cli = fileURLToPath(new URL('../bin/veo.js', import.meta.url));
const run = promisify(execFile);
const { version: pkgVersion } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const sink = () => { let text = ''; return { stdout: { write: chunk => { text += chunk; } }, read: () => text }; };
const veo = (args) => run(process.execPath, [cli, ...args], { env: { ...process.env, VEO_NO_UPDATE_CHECK: '1' } });

const fixtures = [];
test.after(async () => {
  for (const root of fixtures) await rm(root, { recursive: true, force: true });
});

async function changelogFixture(markdown = SAMPLE) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-changes-'));
  fixtures.push(root);
  const file = path.join(root, 'CHANGELOG.md');
  await writeFile(file, markdown);
  return file;
}

const SAMPLE = `# Changelog

All notable changes to veo.

## 1.2.0 - 2026-03-04

### Added

- Added a wrapped entry that continues
  on the next source line.
- Added a parent entry:
  - first nested note
  - second nested note

### Fixed

- Fixed one thing.

## 1.1.0

### Added

- Older entry.

## 1.0.0

### Added

- The first release.

## Appendix

Not a release.
`;

test('the changelog parser keeps sections, wrapped lines and nested bullets', () => {
  const releases = parseChangelog(SAMPLE);
  assert.deepEqual(releases.map(release => release.version), ['1.2.0', '1.1.0', '1.0.0']);
  assert.equal(releases[0].date, '2026-03-04');
  assert.equal(releases[1].date, null);
  const [added] = releases[0].sections;
  assert.equal(added.type, 'Added');
  assert.equal(added.items[0].text, 'Added a wrapped entry that continues on the next source line.');
  assert.deepEqual(added.items[1].items.map(item => item.text), ['first nested note', 'second nested note']);
  // A later "## Appendix" heading is not a version, so its text is not parsed.
  assert.ok(!JSON.stringify(releases).includes('Not a release'));
  assert.deepEqual(releaseCounts(releases[0]), { Added: 2, Fixed: 1 });
  assert.deepEqual(releaseCounts(releases[2]), { Added: 1 });
});

test('the parser ignores the preamble and survives an empty document', () => {
  assert.deepEqual(parseChangelog('# Changelog\n\nNo releases yet.\n'), []);
  assert.deepEqual(parseChangelog(''), []);
});

test('a version resolves exactly, then by prefix, newest match first', () => {
  const releases = parseChangelog(SAMPLE);
  assert.equal(resolveVersion(releases, '1.1.0').version, '1.1.0');
  assert.equal(resolveVersion(releases, 'v1.1.0').version, '1.1.0');
  assert.equal(resolveVersion(releases, '1.0').version, '1.0.0');
  assert.throws(() => resolveVersion(releases, '9.9.9'), /Unknown version "9\.9\.9"\. Known versions: 1\.2\.0, 1\.1\.0, 1\.0\.0\./);
  assert.throws(() => resolveVersion(releases, 'nope'), /version must be a version number/);
  assert.throws(() => resolveVersion([], '1.0.0'), /lists no releases/);
  assert.throws(() => resolveVersion(releases, '1.1.5', '--since'), /Unknown --since/);
});

test('selection covers the list, one release, the newest and every range', () => {
  const releases = parseChangelog(SAMPLE);
  assert.deepEqual(selectReleases(releases).versions.map(release => release.version), ['1.2.0', '1.1.0', '1.0.0']);
  assert.equal(selectReleases(releases, { version: '1.1.0' }).mode, 'version');
  assert.equal(selectReleases(releases, { latest: true }).versions[0].version, '1.2.0');
  assert.deepEqual(selectReleases(releases, { since: '1.0.0' }).versions.map(release => release.version), ['1.2.0', '1.1.0']);
  const range = selectReleases(releases, { since: '1.0.0', to: '1.1.0' });
  assert.deepEqual([range.from, range.to], ['1.0.0', '1.1.0']);
  assert.deepEqual(range.versions.map(release => release.version), ['1.1.0']);
  // --since without --to runs to the newest listed release.
  assert.equal(selectReleases(releases, { since: '1.1.0' }).to, '1.2.0');
  assert.throws(() => selectReleases(releases, { since: '1.2.0' }), /already at 1\.2\.0/);
  assert.throws(() => selectReleases(releases, { since: '1.2.0', to: '1.0.0' }), /--to 1\.0\.0 is older than --since 1\.2\.0/);
});

test('argument parsing accepts both flag styles and rejects contradictions', () => {
  assert.deepEqual(parseChangesArgs(['1.1.0', '--json']), { json: true, latest: false, version: '1.1.0', since: null, to: null });
  assert.equal(parseChangesArgs(['--version=1.1.0']).version, '1.1.0');
  assert.equal(parseChangesArgs(['--since', '1.0.0', '--to', '1.1.0']).to, '1.1.0');
  const range = parseChangesArgs(['--range', '1.0.0..1.1.0']);
  assert.deepEqual([range.since, range.to], ['1.0.0', '1.1.0']);
  assert.throws(() => parseChangesArgs(['--unknown']), /Unknown option "--unknown"/);
  assert.throws(() => parseChangesArgs(['--since']), /--since requires a version number/);
  assert.throws(() => parseChangesArgs(['--range', '1.0.0']), /--range must look like/);
  assert.throws(() => parseChangesArgs(['1.1.0', '--since', '1.0.0']), /either a version or --latest/);
  assert.throws(() => parseChangesArgs(['--latest', '--to', '1.1.0']), /--latest on its own/);
  assert.throws(() => parseChangesArgs(['1.0.0', '1.1.0']), /Usage: veo changes/);
});


test('the list marks the installed version and aligns the columns', async () => {
  const file = await changelogFixture();
  const text = sink();
  assert.equal(await changesMain([], { stdout: text.stdout, current: '1.1.0', file }), 0);
  const lines = text.read().split('\n');
  assert.match(lines[0], /^veo changes \(3 versions, newest first\)$/);
  assert.equal(lines[2], '  1.2.0               Added 2, Fixed 1');
  assert.equal(lines[3], '  1.1.0  (installed)  Added 1');
  // Every summary starts in the same column, whatever the version length is.
  assert.equal(lines[2].indexOf('Added'), lines[3].indexOf('Added'));
  assert.ok(!lines.some(line => /\s$/.test(line)), 'no line may end in spaces');
  assert.match(text.read(), /Use veo changes <version>/);
  const one = sink();
  await changesMain(['1.0.0'], { stdout: one.stdout, current: '1.1.0', file });
  assert.equal(one.read(), 'veo changes 1.0.0\n\nAdded\n  - The first release.\n');
});

test('range output repeats each release heading and marks the installed one', async () => {
  const file = await changelogFixture();
  const text = sink();
  assert.equal(await changesMain(['--since', '1.0.0'], { stdout: text.stdout, current: '1.1.0', file }), 0);
  assert.match(text.read(), /^veo changes 1\.0\.0 to 1\.2\.0 \(2 versions, newest first\)\n\n1\.2\.0\nAdded/);
  assert.match(text.read(), /\n1\.1\.0 {2}\(installed\)\nAdded/);
  assert.ok(!text.read().endsWith('\n\n'), 'no trailing blank line');
});

test('json output keeps the parsed structure for scripts', async () => {
  const file = await changelogFixture();
  const list = sink();
  await changesMain(['--json'], { stdout: list.stdout, current: '1.1.0', file });
  const listed = JSON.parse(list.read());
  assert.equal(listed.current, '1.1.0');
  assert.equal(listed.count, 3);
  assert.deepEqual(listed.versions[0], { version: '1.2.0', date: '2026-03-04', installed: false, counts: { Added: 2, Fixed: 1 }, summary: 'Added 2, Fixed 1' });

  const one = sink();
  await changesMain(['1.2.0', '--json'], { stdout: one.stdout, current: '1.1.0', file });
  const single = JSON.parse(one.read());
  assert.deepEqual(Object.keys(single), ['current', 'version', 'date', 'installed', 'sections']);
  assert.equal(single.installed, false);
  assert.equal(single.sections[0].items[0].text, 'Added a wrapped entry that continues on the next source line.');
  assert.equal(single.sections[0].items[1].items.length, 2);

  const range = sink();
  await changesMain(['--range', '1.0.0..1.1.0', '--json'], { stdout: range.stdout, current: '1.1.0', file });
  const between = JSON.parse(range.read());
  assert.deepEqual([between.from, between.to, between.count], ['1.0.0', '1.1.0', 1]);
  assert.equal(between.versions[0].installed, true);
  assert.ok(between.versions[0].sections.length, 'range entries carry their sections');
});

test('help is printed for -h and --help anywhere in the arguments', async () => {
  for (const args of [['--help'], ['-h'], ['1.1.0', '--help']]) {
    const text = sink();
    assert.equal(await changesMain(args, { stdout: text.stdout, current: '1.1.0', file: 'unused' }), 0);
    assert.equal(text.read(), CHANGES_HELP);
  }
  assert.match(CHANGES_HELP, /veo changes \[<version>\]/);
});

test('long bullets wrap to the terminal width while json stays complete', async () => {
  const long = 'A very long entry that has to be wrapped because it does not fit on one terminal line at all.';
  const file = await changelogFixture(`# Changelog\n\n## 2.0.0\n\n### Added\n\n- ${long}\n`);
  const narrow = sink();
  Object.assign(narrow.stdout, { isTTY: true, columns: 50 });
  assert.equal(await changesMain(['2.0.0', '--no-color'], { stdout: narrow.stdout, current: '2.0.0', file }), 0);
  const wrapped = narrow.read().trimEnd().split('\n');
  assert.ok(wrapped.length > 3, 'the entry wraps over several lines');
  for (const line of wrapped) assert.ok(line.length <= 50, line);
  const json = sink();
  Object.assign(json.stdout, { isTTY: true, columns: 50 });
  await changesMain(['2.0.0', '--json'], { stdout: json.stdout, current: '2.0.0', file });
  assert.equal(JSON.parse(json.read()).sections[0].items[0].text, long);
});

test('a missing or empty changelog is reported instead of crashing', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'veo-changes-'));
  fixtures.push(root);
  await assert.rejects(readChangelog(path.join(root, 'CHANGELOG.md')), /No CHANGELOG\.md found/);
  const empty = path.join(root, 'empty.md');
  await writeFile(empty, '');
  assert.deepEqual(await readChangelog(empty), []);
  const text = sink();
  assert.equal(await changesMain([], { stdout: text.stdout, current: '1.0.0', file: empty }), 0);
  assert.match(text.read(), /No release notes found in CHANGELOG\.md\./);
});

test('the shipped changelog is parsed and its newest entry is the package version', async () => {
  const releases = await readChangelog();
  assert.ok(releases.length > 1);
  assert.equal(releases[0].version, pkgVersion);
  assert.equal(releases[0].sections.length > 0, true);
  assert.equal(selectReleases(releases, { version: pkgVersion }).versions[0].version, pkgVersion);
});

test('the CLI routes changes, its alias and its errors', async () => {
  const listed = JSON.parse((await veo(['changes', '--json'])).stdout);
  assert.equal(listed.current, pkgVersion);
  assert.equal(listed.versions[0].installed, true);
  assert.equal(JSON.parse((await veo(['changelog', '--latest', '--json'])).stdout).version, pkgVersion);
  const range = JSON.parse((await veo(['changes', '--since', '1.0.2', '--json'])).stdout);
  assert.equal(range.from, '1.0.2');
  assert.ok(range.count > 0);
  assert.match((await veo(['changes', '--help'])).stdout, /veo changes \[<version>\]/);
  await assert.rejects(veo(['changes', '9.9.9']), error => error.stderr.includes('Unknown version "9.9.9".'));
  await assert.rejects(veo(['changes', '--sice', '1.9.0']), error => error.stderr.includes('Did you mean "--since"?'));
  await assert.rejects(veo(['chnages', '--json']), error => error.stderr.includes('Did you mean "changes"?'));
  // Release notes already list the versions, so no update hint is printed.
  assert.equal((await veo(['changes', '1.0.2'])).stderr, '');
});
