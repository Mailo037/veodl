import { commandOutput } from './output.js';
import { folderLink } from './path-links.js';
import path from 'node:path';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { configBase } from './paths.js';
import { configSyntaxError } from './config-errors.js';
import { CONFIG_TEMPLATE, CONFIG_GUIDE, stripConfigComments } from './config-template.js';

/**
 * Keys accepted in the config file, with the CLI flag they act as a default for.
 * Everything here is a default only: an explicit command-line flag always wins.
 */
export const CONFIG_KEYS = Object.freeze({
  concurrentDownloads: 'number',
  adaptiveConcurrency: 'boolean',
  filenameTemplate: 'string',
  folderTemplate: 'string',
  checkSpace: 'boolean',
  timings: 'boolean',
  color: 'boolean',
  output: 'string',
  quality: 'string',
  format: 'string',
  compatible: 'boolean',
  recode: 'boolean',
  playlistConcurrency: 'number',
  rename: 'string',
  audio: 'boolean',
  open: 'boolean',
  resume: 'boolean',
  incognito: 'boolean',
  neutralFilename: 'boolean',
  closestQuality: 'boolean',
  cookies: 'string',
  cookiesFromBrowser: 'string',
  playlist: 'boolean',
  concurrentFragments: 'number',
  experimentalFragments: 'boolean',
  subs: 'boolean',
  autoSubs: 'boolean',
  subLangs: 'string',
  subFormat: 'string',
  verify: 'boolean',
  embedSubs: 'boolean',
  embedMetadata: 'boolean',
  embedThumbnail: 'boolean',
  sponsorblockRemove: 'string',
  section: 'string',
  json: 'boolean',
  skipExisting: 'boolean',
  playlistItems: 'string',
  deepScan: 'boolean',
  timeout: 'string',
  listSources: 'boolean',
  autoListSources: 'boolean',
});

export const RESERVED_PROFILE_NAMES = Object.freeze(['list', 'reset']);

export function configFile({ env = process.env, platform = process.platform } = {}) {
  const override = env.VEO_CONFIG;
  if (typeof override === 'string' && override.trim()) {
    if (override.includes('\0')) throw new Error('VEO_CONFIG must be a filesystem path.');
    return path.resolve(override.trim());
  }
  return path.join(configBase({ env, platform }), 'config.json');
}

function typeOf(value) {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}

/**
 * Read optional user defaults. A missing file is not an error; a malformed one
 * is reported loudly, because silently ignoring a typo would be worse.
 */
export async function loadConfig({ env = process.env, file } = {}) {
  const target = file || configFile({ env });
  let text;
  try {
    text = await readFile(target, 'utf8');
  } catch (error) {
    if (error.code === 'ENOENT') return { file: target, exists: false, config: {}, warnings: [] };
    throw new Error(`Cannot read the veo config file ${target}: ${error.message}`);
  }
  return parseConfigText(text, target);
}

export function parseConfigText(text, target = 'config') {
  let data;
  let clean = text.replace(/^\uFEFF/, '');
  try {
    clean = stripConfigComments(text);
    data = JSON.parse(clean);
  } catch (error) {
    throw configSyntaxError(clean, target, error);
  }
  if (!data || typeof data !== 'object' || Array.isArray(data)) {
    throw new Error(`The veo config file must contain a JSON object: ${target}`);
  }
  const warnings = [];
  const config = {};
  for (const [key, value] of Object.entries(data)) {
    if (key === '$schema') continue;
    if (key === 'activeProfile') {
      if (typeOf(value) !== 'string' || !value) throw new Error(`Config key "activeProfile" must be a non-empty string (${target}).`);
      config.activeProfile = value;
      continue;
    }
    if (key === 'profiles') {
      if (!value || typeOf(value) !== 'object') throw new Error('Config profiles must be an object.');
      config.profiles = Object.create(null);
      for (const [name, profile] of Object.entries(value)) {
        if (RESERVED_PROFILE_NAMES.includes(name)) throw new Error(`Profile name "${name}" is reserved for veo profile ${name}. Rename it in ${target}.`);
        if (!profile || typeOf(profile) !== 'object') throw new Error(`Profile "${name}" must be an object.`);
        const checked = {};
        for (const [setting, item] of Object.entries(profile)) {
          if (!Object.hasOwn(CONFIG_KEYS, setting)) throw new Error(`Unknown setting "${setting}" in profile "${name}".`);
          if (typeOf(item) !== CONFIG_KEYS[setting]) throw new Error(`Profile "${name}": "${setting}" must be a ${CONFIG_KEYS[setting]}.`);
          checked[setting] = item;
        }
        config.profiles[name] = checked;
      }
      continue;
    }
    const expected = CONFIG_KEYS[key];
    if (!expected) {
      warnings.push(`Unknown config key "${key}" in ${target} was ignored.`);
      continue;
    }
    if (typeOf(value) !== expected) {
      throw new Error(`Config key "${key}" must be a ${expected}, not a ${typeOf(value)} (${target}).`);
    }
    config[key] = value;
  }
  if (config.activeProfile && !Object.hasOwn(config.profiles || {}, config.activeProfile)) {
    throw new Error(`Unknown active profile "${config.activeProfile}" in ${target}. Available: ${Object.keys(config.profiles || {}).join(', ') || 'none'}.`);
  }
  return { file: target, exists: true, config, warnings };
}

export function selectedProfileName(config, name) {
  return name || config.activeProfile || (Object.hasOwn(config.profiles || {}, 'default') ? 'default' : undefined);
}

export function applyProfile(config, name) {
  const { profiles, activeProfile, ...defaults } = config;
  name = selectedProfileName(config, name);
  if (!name) return defaults;
  if (!profiles || !Object.hasOwn(profiles, name)) throw new Error(`Unknown profile "${name}". Available: ${Object.keys(profiles || {}).join(', ') || 'none'}. Use veo config edit.`);
  return { ...defaults, ...profiles[name] };
}

export async function profileMain(args, { file = configFile(), output = process.stdout } = {}) {
  if (args.length > 1 || args[0]?.startsWith('-')) throw new Error('Usage: veo profile [list|reset|NAME]');
  const loaded = await loadConfig({ file });
  if (!args.length) {
    output.write(`Active profile: ${selectedProfileName(loaded.config) || 'global (no profile)'}\n`);
    output.write(`Available: ${Object.keys(loaded.config.profiles || {}).join(', ') || 'none'}\n`);
    return 0;
  }
  if (args[0] === 'list' && args.length === 1) {
    const names = Object.keys(loaded.config.profiles || {});
    if (!names.length) output.write('No profiles configured. Use veo config edit.\n');
    else {
      const active = selectedProfileName(loaded.config);
      output.write(`Profiles:\n${active ? '' : '* global (no profile)\n'}${names.map(name => `${name === active ? '* ' : '  '}${name}${name === active ? ' (active)' : ''}`).join('\n')}\n`);
    }
    return 0;
  }
  const reset = args[0] === 'reset' && args.length === 1;
  const name = args[0];
  if (!reset) {
    applyProfile(loaded.config, name);
    await effectiveConfig(loaded.config, name);
  }
  if (!loaded.exists) {
    output.write('Active profile: global (no profile)\n');
    return 0;
  }
  const original = await readFile(file, 'utf8');
  // Token locations come from the comment-aware parser, preserving hand-written notes.
  const { configSource } = await import('./config-diagnostics.js');
  const { nodes } = configSource(original);
  const property = nodes.get(JSON.stringify(['activeProfile']));
  let updated;
  if (reset) {
    if (!property) updated = original;
    else {
      const clean = (original.startsWith('\uFEFF') ? ' ' : '') + stripConfigComments(original);
      let start = property.keyStart, end = property.end;
      while (/\s/.test(clean[end] || '')) end++;
      if (clean[end] === ',') end++;
      else {
        let before = start - 1;
        while (before >= 0 && /\s/.test(clean[before])) before--;
        if (clean[before] === ',') start = before;
        end = property.end;
      }
      updated = original.slice(0, start) + original.slice(end);
    }
  } else if (property) updated = original.slice(0, property.start) + JSON.stringify(name) + original.slice(property.end);
  else {
    const root = nodes.get('[]');
    const clean = (original.startsWith('\uFEFF') ? ' ' : '') + stripConfigComments(original);
    const first = root.start + 1 + clean.slice(root.start + 1, root.end - 1).search(/\S/);
    const empty = first === root.start;
    const newline = original.includes('\r\n') ? '\r\n' : '\n';
    const position = empty ? root.end - 1 : first;
    const indent = empty ? '  ' : /^[ \t]*$/.test(original.slice(original.lastIndexOf('\n', position - 1) + 1, position))
      ? original.slice(original.lastIndexOf('\n', position - 1) + 1, position) : '';
    updated = original.slice(0, position) + (empty ? `${newline}${indent}` : '') +
      `"activeProfile": ${JSON.stringify(name)}${empty ? `${newline}` : `,${newline}${indent}`}` + original.slice(position);
  }
  parseConfigText(updated, file);
  if (updated !== original) {
    if (await readFile(file, 'utf8') !== original) throw new Error('Config changed while switching profiles. Try again.');
    await writeFile(file, updated, 'utf8');
  }
  output.write(`Active profile: ${reset ? selectedProfileName(parseConfigText(updated, file).config) || 'global (no profile)' : name}\n`);
  return 0;
}

export async function prepareConfigEdit(file) {
  await mkdir(path.dirname(file), { recursive: true });
  try { await writeFile(file, CONFIG_TEMPLATE, { flag: 'wx', mode: 0o600 }); }
  catch (error) {
    if (error.code !== 'EEXIST') throw error;
    const original = await readFile(file, 'utf8');
    if (!original.replace(/^\uFEFF/, '').trim()) await writeFile(file, CONFIG_TEMPLATE, 'utf8');
  }
}

export async function configMain(args) {
  const editorColor = !args.includes('--no-color') && !Object.hasOwn(process.env, 'NO_COLOR');
  let stdout;
  [args, stdout] = commandOutput(args, process.stdout);
  if (args[0] === 'show' || args[0] === 'guide') stdout = process.stdout;
  const file = configFile();
  if (args[0] === 'reset') {
    if (args.length !== 1) throw new Error('Usage: veo config reset');
    return (await import('./config-reset.js')).resetConfig(file, { output: stdout });
  }
  if (['check', 'show'].includes(args[0])) {
    const { parseArgs } = await import('node:util');
    const { values, positionals } = parseArgs({ args: args.slice(1), allowPositionals: true, options: { profile: { type: 'string' } } });
    if (positionals.length) throw new Error('Usage: veo config check|show [--profile NAME]');
    const loaded = await loadConfig({ file });
    if (loaded.warnings.length) throw new Error(loaded.warnings.join('\n'));
    if (args[0] === 'show') {
      const effective = await effectiveConfig(loaded.config, values.profile);
      stdout.write(`${JSON.stringify(effective, null, 2)}\n`);
    } else {
      const names = values.profile ? [values.profile] : [undefined, ...Object.keys(loaded.config.profiles || {})];
      for (const name of names) {
        try { await effectiveConfig(loaded.config, name); }
        catch (error) { throw new Error(`Profile ${name || 'default/global'}: ${error.message}`); }
      }
      stdout.write(`Config OK: ${file} (${names.length} effective configurations checked).\n`);
    }
    return 0;
  }
  let editorMode;
  if (args[0] === 'edit') {
    if (args.length > 2 || (args[1] && !['--external', '--terminal'].includes(args[1]))) throw new Error('Usage: veo config edit [--external|--terminal]');
    editorMode = args[1]?.slice(2) || process.env.VEO_CONFIG_EDITOR || 'auto';
    if (!['auto', 'external', 'terminal'].includes(editorMode)) throw new Error('VEO_CONFIG_EDITOR must be auto, external or terminal.');
  } else if (args.length !== 1 || !['path', 'profiles', 'guide'].includes(args[0])) throw new Error('Usage: veo config edit|path|profiles|guide|check|show|reset');
  if (args[0] === 'path') { stdout.write(`${folderLink(stdout, file)}\n`); return 0; }
  if (args[0] === 'guide') { stdout.write(CONFIG_GUIDE); return 0; }
  if (args[0] === 'profiles') {
    const loaded = await loadConfig();
    stdout.write(`${Object.keys(loaded.config.profiles || {}).join('\n') || 'No profiles configured. Use veo config edit.'}\n`);
    return 0;
  }
  await prepareConfigEdit(file);
  if (editorMode === 'terminal' || (editorMode === 'auto' && !process.env.VISUAL && !process.env.EDITOR && process.stdin.isTTY && process.stdout.isTTY)) {
    const { editConfig } = await import('./config-editor.js');
    await editConfig(file, { color: editorColor });
    return 0;
  }
  // Treat the editor as an executable path, never as shell code.
  const editor = process.env.VISUAL || process.env.EDITOR || (process.platform === 'win32' ? 'notepad.exe' : 'vi');
  await new Promise((resolve, reject) => {
    const child = spawn(editor, [file], { shell: false, stdio: 'inherit', windowsHide: true });
    child.on('error', () => reject(new Error(`Could not launch editor. Set EDITOR to an executable path or edit ${file}.`)));
    child.on('close', code => code === 0 ? resolve() : reject(new Error(`Editor exited with ${code}. Config: ${file}`)));
  });
  await loadConfig();
  return 0;
}

export async function effectiveConfig(config, profile) {
  const { parseCli } = await import('./cli.js');
  const selected = applyProfile(config, profile);
  // Inspection is offline and never prints credential file paths or browser profiles.
  const { cookies, cookiesFromBrowser, ...safe } = selected;
  const parsed = parseCli(['https://example.invalid/config-check'], { config: safe });
  const result = Object.fromEntries(Object.keys(CONFIG_KEYS).filter(key => parsed[key] !== undefined).map(key => [key, parsed[key]]));
  result.output = path.resolve(result.output);
  if (cookies !== undefined) result.cookies = '[configured]';
  if (cookiesFromBrowser !== undefined) result.cookiesFromBrowser = '[configured]';
  return result;
}
