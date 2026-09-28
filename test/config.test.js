import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { configFile, loadConfig, prepareConfigEdit, profileMain, CONFIG_KEYS } from '../src/config.js';
import { CONFIG_TEMPLATE, CONFIG_GUIDE, addConfigGuide, removeConfigGuide, configGuideState, stripConfigComments } from '../src/config-template.js';
import { parseCli } from '../src/cli.js';

test('the config location follows each platform convention and VEO_CONFIG', () => {
  assert.equal(configFile({ env: { VEO_CONFIG: './custom.json' } }), path.resolve('./custom.json'));
  assert.match(configFile({ env: {} }), /config\.json$/);
  assert.throws(() => configFile({ env: { VEO_CONFIG: 'bad\0path' } }), /filesystem path/);
  assert.ok(configFile({ env: { VEO_CONFIG: '   ' } }).endsWith(path.join('veo', 'config.json')));
});

test('a missing config file is not an error', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-config-'));
  try {
    const loaded = await loadConfig({ file: path.join(directory, 'absent.json') });
    assert.equal(loaded.exists, false);
    assert.deepEqual(loaded.config, {});
    assert.deepEqual(loaded.warnings, []);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('valid defaults load while unknown keys warn and are ignored', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-config-'));
  const file = path.join(directory, 'config.json');
  try {
    await writeFile(file, JSON.stringify({
      $schema: 'https://example.com/veo.json',
      quality: '720p',
      output: './videos',
      audio: false,
      concurrentFragments: 4,
      subLangs: 'de,en',
      nope: true,
    }));
    const loaded = await loadConfig({ file });
    assert.equal(loaded.exists, true);
    assert.equal(loaded.config.quality, '720p');
    assert.equal(loaded.config.output, './videos');
    assert.equal(loaded.config.concurrentFragments, 4);
    assert.equal(loaded.config.subLangs, 'de,en');
    assert.equal(Object.hasOwn(loaded.config, 'nope'), false);
    assert.equal(Object.hasOwn(loaded.config, '$schema'), false);
    assert.equal(loaded.warnings.length, 1);
    assert.match(loaded.warnings[0], /Unknown config key "nope"/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('a malformed or mistyped config file fails loudly', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-config-'));
  const file = path.join(directory, 'config.json');
  try {
    await writeFile(file, '{ not json');
    await assert.rejects(loadConfig({ file }), /not valid JSON/);
    await writeFile(file, '"a string"');
    await assert.rejects(loadConfig({ file }), /must contain a JSON object/);
    await writeFile(file, '["a"]');
    await assert.rejects(loadConfig({ file }), /must contain a JSON object/);
    await writeFile(file, JSON.stringify({ quality: 720 }));
    await assert.rejects(loadConfig({ file }), /"quality" must be a string, not a number/);
    await writeFile(file, JSON.stringify({ audio: 'yes' }));
    await assert.rejects(loadConfig({ file }), /"audio" must be a boolean/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('every documented key maps to a supported type', () => {
  for (const [key, type] of Object.entries(CONFIG_KEYS)) {
    assert.ok(['string', 'boolean', 'number'].includes(type), key);
    assert.ok(CONFIG_GUIDE.includes(JSON.stringify(key)), `${key} is missing from the optional guide`);
  }
  assert.ok(Object.keys(CONFIG_KEYS).length >= 15);
});

test('incognito can be selected by profile and overridden for one request', () => {
  const config = { profiles: { private: { incognito: true } }, activeProfile: 'private' };
  const url = 'https://example.test/video';
  assert.equal(parseCli([url], { config }).incognito, true);
  assert.equal(parseCli([url, '--no-incognito'], { config }).incognito, false);
  assert.equal(parseCli([url], { config: { resume: true, skipExisting: true, ...config } }).resume, false);
  assert.equal(parseCli([url], { config: { resume: true, skipExisting: true, ...config } }).skipExisting, false);
  assert.throws(() => parseCli([url, '--resume'], { config }), /cannot be combined with --resume/);
  assert.throws(() => parseCli([url, '--skip-existing'], { config }), /cannot be combined with --skip-existing/);
  assert.throws(() => parseCli(['--retry-failed', 'abc123'], { config }), /cannot retry/);
});

test('neutral filenames are optional in a private profile', () => {
  const url = 'https://example.test/video';
  const config = { profiles: { private: { incognito: true, neutralFilename: true } }, activeProfile: 'private' };
  assert.equal(parseCli([url, '--no-neutral-filename'], { config }).neutralFilename, false);
  assert.equal(parseCli([url], { config }).neutralFilename, true);
  assert.equal(parseCli([url, '--neutral-filename']).neutralFilename, true);
  assert.equal(parseCli([url]).neutralFilename, false);
  assert.equal(parseCli([url, '--neutral-filename', '--rename', 'Secret', '--filename-template', '{title}']).neutralFilename, true);
});

test('source discovery config defaults are validated and explicit CLI flags override them', async () => {
  const settings = { deepScan: true, timeout: '2m', listSources: true, autoListSources: true };
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-source-config-'));
  const file = path.join(directory, 'config.json');
  try {
    await writeFile(file, JSON.stringify(settings));
    const { config } = await loadConfig({ file });
    const url = 'https://example.test/movie/12';
    const defaults = parseCli([url], { config });
    assert.equal(defaults.deepScan, true);
    assert.equal(defaults.timeoutMs, 120000);
    assert.equal(defaults.listSources, true);
    assert.equal(defaults.autoListSources, true);
    const overrides = parseCli([url, '--no-deep-scan', '--timeout', '30s', '--no-list-sources', '--no-auto-list-sources'], { config });
    assert.equal(overrides.deepScan, false);
    assert.equal(overrides.timeoutMs, 30000);
    assert.equal(overrides.listSources, false);
    assert.equal(overrides.autoListSources, false);
    assert.equal(parseCli([url, '--source', '1'], { config }).listSources, false);
    assert.equal(parseCli(['--retry-failed', 'job.json'], { config }).listSources, false);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('invalid config reports the file, exact location and actionable syntax reason', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-errors-'));
  const file = path.join(directory, 'config.json');
  try {
    for (const [text, reason, location] of [
      [String.raw`{
"output": "G:\Meine Ablage\Filme"
}`, /Invalid escape sequence.*forward slashes/, /line 2, column 15/],
      ['{\n```json\n"audio": true\n```\n}', /Markdown code fences/, /line 2, column 1/],
      ['{\n"audio": true,\n}', /Trailing commas/, /line 3, column 1/],
      ['{\n"audio" true\n}', /Expected a colon/, /line 2, column 9/],
      ['{\n"audio": true\n"open": false}', /Expected a comma/, /line 3, column 1/],
      ['\uFEFF{\r\n// note\r\n"audio": true,\r\n}', /Trailing commas/, /line 4, column 1/],
      ['{\n/* unfinished', /Unterminated block comment/, /line 2, column 1/],
      ['{"audio": true', /closing }/, /line 1, column 15/],
    ]) {
      await writeFile(file, text);
      await assert.rejects(loadConfig({ file }), error => {
        assert.ok(error.message.includes(file));
        assert.match(error.message, reason);
        assert.match(error.message, location);
        return true;
      });
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('config edit leaves existing files untouched and creates a small new config', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-template-'));
  const file = path.join(directory, 'config.json');
  try {
    await prepareConfigEdit(file);
    assert.equal(await readFile(file, 'utf8'), CONFIG_TEMPLATE);
    assert.deepEqual((await loadConfig({ file })).config.profiles.default, {});
    assert.equal((await loadConfig({ file })).config.profiles.music.audio, true);
    await writeFile(file, '\uFEFF  \r\n');
    await prepareConfigEdit(file);
    assert.equal(await readFile(file, 'utf8'), CONFIG_TEMPLATE);
    const original = '{\n  "quality": "480p",\n  "open": true // keep my comment\n}\n';
    await writeFile(file, original);
    await prepareConfigEdit(file);
    assert.equal(await readFile(file, 'utf8'), original);
    assert.equal((await loadConfig({ file })).config.quality, '480p');
    await prepareConfigEdit(file);
    assert.equal(await readFile(file, 'utf8'), original);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('profile command names cannot be used as config profile names', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-reserved-profile-'));
  const file = path.join(directory, 'config.json');
  try {
    for (const name of ['list', 'reset']) {
      await writeFile(file, JSON.stringify({ profiles: { [name]: { quality: 'best' } } }));
      await assert.rejects(loadConfig({ file }), new RegExp(`Profile name "${name}" is reserved`));
    }
    await writeFile(file, JSON.stringify({ profiles: { default: {}, List: {} } }));
    assert.deepEqual(Object.keys((await loadConfig({ file })).config.profiles), ['default', 'List']);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('profile command persists the active profile without removing comments or other settings', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-profile-switch-'));
  const file = path.join(directory, 'config.json');
  const original = '\uFEFF// personal note\r\n{\r\n  "quality": "720p",\r\n  "profiles": { "default": {}, "music": { "audio": true } }\r\n}\r\n';
  const output = { text: '', write(value) { this.text += value; } };
  try {
    await writeFile(file, original);
    assert.equal(await profileMain([], { file, output }), 0);
    assert.match(output.text, /Active profile: default/);
    await assert.rejects(profileMain(['missing'], { file, output }), /Unknown profile/);
    assert.equal(await readFile(file, 'utf8'), original);
    await profileMain(['music'], { file, output });
    const changed = await readFile(file, 'utf8');
    assert.ok(changed.startsWith('\uFEFF// personal note\r\n'));
    assert.match(changed, /"activeProfile": "music"/);
    assert.match(changed, /"quality": "720p"/);
    assert.equal(parseCli(['https://example.test/video'], { config: (await loadConfig({ file })).config }).audio, true);
    assert.equal(parseCli(['https://example.test/video'], { config: (await loadConfig({ file })).config }).profile, 'music');
    assert.equal(parseCli(['https://example.test/video', '--profile', 'default'], { config: (await loadConfig({ file })).config }).audio, false);
    output.text = '';
    await profileMain(['list'], { file, output });
    assert.match(output.text, /\* music \(active\)/);
    assert.match(output.text, /  default/);
    await profileMain(['default'], { file, output });
    assert.equal((await loadConfig({ file })).config.activeProfile, 'default');
    assert.equal((await readFile(file, 'utf8')).split('activeProfile').length, 2, 'switch updates one property');
    await profileMain(['reset'], { file, output });
    assert.equal((await loadConfig({ file })).config.activeProfile, undefined);
    assert.equal(parseCli(['https://example.test/video'], { config: (await loadConfig({ file })).config }).profile, 'default');
    assert.match(await readFile(file, 'utf8'), /personal note/);
    output.text = '';
    await profileMain(['list'], { file, output });
    assert.match(output.text, /\* default \(active\)/);
    await writeFile(file, '{"profiles":{"music":{"audio":true}},"activeProfile":"music"}');
    await profileMain(['reset'], { file, output });
    assert.equal((await loadConfig({ file })).config.activeProfile, undefined);
    assert.equal(parseCli(['https://example.test/video'], { config: (await loadConfig({ file })).config }).profile, undefined);
    assert.doesNotMatch(await readFile(file, 'utf8'), /activeProfile/);
    output.text = '';
    await profileMain(['list'], { file, output });
    assert.match(output.text, /\* global \(no profile\)/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('generated config guides are optional, replaceable and removable without changing settings', () => {
  const original = '// My note\n{"quality":"best","profiles":{"music":{"audio":true}}}\n';
  const guided = addConfigGuide(original);
  assert.equal(configGuideState(original), 'absent');
  assert.equal(configGuideState(guided), 'current');
  assert.deepEqual(JSON.parse(stripConfigComments(guided)), JSON.parse(stripConfigComments(original)));
  assert.equal(removeConfigGuide(guided), original);
  assert.equal(addConfigGuide(guided), guided);
  const stale = guided.replace(/(veo: template reference begin) [a-f0-9]{12}/, '$1 old');
  assert.equal(configGuideState(stale), 'outdated');
  assert.equal(addConfigGuide(stale), guided);
  const incomplete = guided.replace('// veo: template reference end', '// missing end');
  assert.equal(configGuideState(incomplete), 'incomplete');
  assert.equal(addConfigGuide(incomplete), incomplete);
  assert.equal(removeConfigGuide(incomplete), incomplete);
  const duplicated = '// veo: commented configuration template\n' + CONFIG_GUIDE +
    '// Save and close the editor. Settings apply the next time you run veo.\n' + original;
  assert.equal(removeConfigGuide(duplicated), '// Save and close the editor. Settings apply the next time you run veo.\n' + original);
  const legacy = '// veo: commented configuration template\n// Reference: copy any examples below.\n// old examples\n// Your existing settings:\n' + original;
  assert.equal(configGuideState(legacy), 'outdated');
  assert.equal(removeConfigGuide(legacy), original);
  const bom = '\uFEFF' + original;
  assert.equal(removeConfigGuide(addConfigGuide(bom)), bom);
});
test('comment parsing preserves URLs, escaped quotes and Windows paths', () => {
  const data = { output: 'C:\\Videos\\', rename: 'https://example.test/a//b/*text*/ "quoted"', quality: '720p' };
  const text = `\uFEFF/* settings */\n${JSON.stringify(data, null, 2)} // end\n`;
  assert.deepEqual(JSON.parse(stripConfigComments(text)), data);
  assert.deepEqual(JSON.parse(stripConfigComments('{"audio": /* disabled */ false}')), { audio: false });
  assert.throws(() => stripConfigComments('{} /* unfinished'), /Unterminated/);
  assert.throws(() => JSON.parse(stripConfigComments('{"audio": tr/* comment */ue}')));
  assert.throws(() => JSON.parse(stripConfigComments('{"audio": false,}')));
});

test('old personal comments and profile names are preserved when opening the editor', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-legacy-'));
  const file = path.join(directory, 'config.json');
  const legacy = '// Meine Notiz\n{"profiles":{"musik":{"audio":true}}}\n';
  try {
    await writeFile(file, legacy);
    await prepareConfigEdit(file);
    assert.equal(await readFile(file, 'utf8'), legacy);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
