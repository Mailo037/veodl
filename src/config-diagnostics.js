import { CONFIG_KEYS, RESERVED_PROFILE_NAMES, parseConfigText, effectiveConfig, applyProfile } from './config.js';
import { stripConfigComments } from './config-template.js';
import { QUALITIES, AUDIO_FORMATS, VIDEO_FORMATS } from './utils.js';

// Parse only after JSON.parse succeeds. Keep paths and source spans, including
// escaped property names and repeated settings in different profiles.
export function configSource(text) {
  const clean = stripConfigComments(text);
  const data = JSON.parse(clean), nodes = new Map();
  const tokens = [...clean.matchAll(/"(?:\\.|[^"\\])*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null|[{}\[\]:,]/g)];
  let index = 0;
  const bom = text.startsWith('\uFEFF') ? 1 : 0;
  const visit = (path, keyToken) => {
    const first = tokens[index++];
    if (first[0] === '{') {
      while (tokens[index][0] !== '}') {
        const key = tokens[index++]; index++; // colon
        visit([...path, JSON.parse(key[0])], key);
        if (tokens[index][0] === ',') index++;
      }
      index++;
    } else if (first[0] === '[') {
      let item = 0;
      while (tokens[index][0] !== ']') {
        visit([...path, item++]);
        if (tokens[index][0] === ',') index++;
      }
      index++;
    }
    const last = tokens[index - 1];
    nodes.set(JSON.stringify(path), { path, start: first.index + bom, end: last.index + last[0].length + bom,
      keyStart: keyToken?.index + bom, keyEnd: keyToken ? keyToken.index + keyToken[0].length + bom : undefined });
  };
  visit([]);
  return { data, nodes };
}

function distance(a, b) {
  let row = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 0; i < a.length; i++) {
    const next = [i + 1];
    for (let j = 0; j < b.length; j++) next.push(Math.min(next[j] + 1, row[j + 1] + 1, row[j] + (a[i] !== b[j])));
    row = next;
  }
  return row[b.length];
}

const keys = Object.keys(CONFIG_KEYS);
function suggestion(key, choices) {
  const ranked = choices.map(value => [value, distance(key.toLowerCase(), value.toLowerCase())]).sort((a, b) => a[1] - b[1]);
  return ranked[0]?.[1] <= 2 ? ranked[0][0] : null;
}

function located(text, node, message, property = false, choices = []) {
  const start = (property ? node?.keyStart : node?.start) ?? 0;
  const end = (property ? node?.keyEnd : node?.end) ?? start + 1;
  return { message, start, end, line: text.slice(0, start).split('\n').length,
    column: start - text.lastIndexOf('\n', start - 1), choices };
}

function valueChoices(key, config = {}) {
  if (CONFIG_KEYS[key] === 'boolean') return [true, false];
  if (key === 'quality') return QUALITIES;
  if (key === 'format') return config.audio ? AUDIO_FORMATS : VIDEO_FORMATS;
  if (key === 'timeout') return ['30s', '45s', '1m', '2m', '5m'];
  if (['concurrentDownloads', 'playlistConcurrency'].includes(key)) return [1, 2, 3, 4];
  if (key === 'concurrentFragments') return Array.from({ length: 64 }, (_, i) => i + 1);
  return [];
}

export async function semanticIssue(text) {
  const { data, nodes } = configSource(text);
  const node = path => nodes.get(JSON.stringify(path));
  const object = value => value && typeof value === 'object' && !Array.isArray(value);
  if (!object(data)) return located(text, node([]), 'The config must contain a JSON object.');
  const check = (settings, path) => {
    for (const [key, value] of Object.entries(settings)) {
      if (!path.length && ['$schema', 'profiles', 'activeProfile'].includes(key)) continue;
      const target = node([...path, key]);
      if (!Object.hasOwn(CONFIG_KEYS, key)) {
        const proposed = suggestion(key, path.length ? keys : [...keys, 'profiles', '$schema']);
        return located(text, target, `Unknown setting "${key}".${proposed ? ` Did you mean "${proposed}"?` : ''}`, true, proposed ? [proposed] : []);
      }
      if (typeof value !== CONFIG_KEYS[key]) {
        const choices = valueChoices(key, settings);
        return located(text, target, `"${key}" must be a ${CONFIG_KEYS[key]}.${choices.length ? ` Choose: ${choices.join(', ')}.` : ''}`, false, choices);
      }
    }
    return null;
  };
  let issue = check(data, []);
  if (issue) return issue;
  if (Object.hasOwn(data, 'profiles')) {
    if (!object(data.profiles)) return located(text, node(['profiles']), 'Profiles must be an object.');
    for (const [name, profile] of Object.entries(data.profiles)) {
      if (RESERVED_PROFILE_NAMES.includes(name)) {
        return located(text, node(['profiles', name]), `Profile name "${name}" is reserved for veo profile ${name}. Rename this profile.`, true);
      }
      if (!object(profile)) return located(text, node(['profiles', name]), `Profile "${name}" must be an object.`);
      issue = check(profile, ['profiles', name]); if (issue) return issue;
    }
  }
  if (Object.hasOwn(data, 'activeProfile')) {
    if (typeof data.activeProfile !== 'string' || !data.activeProfile) return located(text, node(['activeProfile']), '"activeProfile" must be a non-empty profile name.');
    if (!object(data.profiles) || !Object.hasOwn(data.profiles, data.activeProfile)) {
      return located(text, node(['activeProfile']), `Unknown active profile "${data.activeProfile}". Choose: ${Object.keys(data.profiles || {}).join(', ') || 'none'}.`, false, Object.keys(data.profiles || {}));
    }
  }
  const { config } = parseConfigText(text);
  for (const profile of [undefined, ...Object.keys(config.profiles || {})]) {
    try { await effectiveConfig(config, profile); }
    catch (error) {
      const message = error.message;
      let key = /Invalid quality/.test(message) ? 'quality' : /Invalid (?:audio|video) format/.test(message) ? 'format' :
        /output directory/.test(message) ? 'output' : /custom filename/.test(message) ? 'rename' :
          /--([a-z-]+)/.exec(message)?.[1]?.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      const name = profile ?? config.activeProfile ?? (config.profiles?.default ? 'default' : undefined);
      const selected = applyProfile(config, profile);
      // Template errors do not name the CLI option. Locate the failing template.
      if (!key) {
        const { validateTemplate } = await import('./naming.js');
        for (const candidate of ['filenameTemplate', 'folderTemplate']) {
          if (selected[candidate] !== undefined) {
            try { validateTemplate(selected[candidate], { folders: candidate === 'folderTemplate' }); }
            catch { key = candidate; break; }
          }
        }
      }
      const target = name !== undefined && Object.hasOwn(config.profiles[name], key) ? node(['profiles', name, key]) : node([key]);
      const value = ['quality', 'format'].includes(key) ? `"${key}": unsupported value ${JSON.stringify(selected[key])}. ` : '';
      return located(text, target || node([]), `${name === undefined ? '' : `Profile "${name}": `}${value}${message}`, false, valueChoices(key, selected));
    }
  }
  return null;
}

export function editorCompletions(text, cursor) {
  try {
    const { data, nodes } = configSource(text);
    for (const item of nodes.values()) {
      const path = item.path;
      if (!(path.length === 1 || (path.length === 3 && path[0] === 'profiles'))) continue;
      const key = path.at(-1), property = cursor >= item.keyStart && cursor <= item.keyEnd;
      if (!property && !(cursor >= item.start && cursor <= item.end)) continue;
      const profile = path.length === 3 ? path[1] : undefined;
      const choices = property ? (path.length === 1 ? [...keys, 'profiles', 'activeProfile', '$schema'] : keys) :
        key === 'activeProfile' ? Object.keys(data.profiles || {}) : valueChoices(key, applyProfile(data, profile));
      if (!choices.length) return null;
      const proposed = property ? suggestion(key, choices) : null;
      return { start: property ? item.keyStart : item.start, end: property ? item.keyEnd : item.end,
        choices: proposed ? [proposed, ...choices.filter(value => value !== proposed)] : choices };
    }
  } catch { /* Syntax must be valid before offering safe token replacements. */ }
  return null;
}
