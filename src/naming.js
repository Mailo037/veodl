import path from 'node:path';
import { lstat, mkdir } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import { sanitizeTitle } from './utils.js';

const FIELDS = new Set(['title', 'id', 'channel', 'year', 'playlist', 'index']);
export function validateTemplate(template, { folders = false } = {}) {
  if (typeof template !== 'string' || !template.trim()) throw new Error('A naming template cannot be empty.');
  if (path.win32.isAbsolute(template) || path.posix.isAbsolute(template) || /[:\\\x00-\x1f]/.test(template)) throw new Error('Templates must be relative and use / for subfolders.');
  if (!folders && template.includes('/')) throw new Error('Use --folder-template for subfolders.');
  if (template.split('/').some(part => !part.trim() || ['.', '..'].includes(part.trim()))) throw new Error('Templates cannot contain empty, . or .. path components.');
  const rest = template.replace(/\{([^{}]+)\}/g, (_, field) => {
    if (!FIELDS.has(field)) throw new Error(`Unknown template field: ${field}. Use ${[...FIELDS].map(key => `{${key}}`).join(', ')}.`);
    return '';
  });
  if (/[{}]/.test(rest)) throw new Error('Unbalanced braces in naming template.');
  return template;
}

export function mediaDestination(options, metadata, fallbackTitle) {
  if (options.neutralFilename) {
    return { directory: path.resolve(options.output), title: `video-${randomBytes(6).toString('hex')}` };
  }
  const fields = {
    title: metadata.title || metadata.id || 'video', id: metadata.id || 'unknown',
    channel: metadata.channel || metadata.uploader || 'Unknown channel',
    year: /^\d{4}/.exec(metadata.upload_date || '')?.[0] || 'Unknown year',
    playlist: options._playlistTitle || metadata.playlist_title || metadata.playlist || 'No playlist',
    index: String(options._entryIndex || metadata.playlist_index || 1).padStart(3, '0'),
  };
  const expand = template => template.replace(/\{([^{}]+)\}/g, (_, field) => sanitizeTitle(String(fields[field])));
  const title = options.filenameTemplate ? sanitizeTitle(expand(validateTemplate(options.filenameTemplate))) : fallbackTitle;
  const folders = options.folderTemplate ? validateTemplate(options.folderTemplate, { folders: true }).split('/').map(part => sanitizeTitle(expand(part))) : [];
  return { directory: path.resolve(options.output, ...folders), title };
}

// Refuse symlink/junction subfolders: metadata must never redirect a save outside output.
export async function prepareDestination(root, directory, { create = false } = {}) {
  root = path.resolve(root); directory = path.resolve(directory);
  const relative = path.relative(root, directory);
  if (relative.startsWith('..' + path.sep) || relative === '..' || path.isAbsolute(relative)) throw new Error('Destination escapes output directory.');
  if (create) await mkdir(root, { recursive: true });
  let current = root;
  for (const component of relative.split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    const info = await lstat(current).catch(error => { if (error.code === 'ENOENT') return null; throw error; });
    if (info && (!info.isDirectory() || info.isSymbolicLink())) throw new Error(`Output subfolder is not a plain directory: ${current}`);
    if (!info && create) {
      await mkdir(current).catch(error => { if (error.code !== 'EEXIST') throw error; });
      const created = await lstat(current);
      if (!created.isDirectory() || created.isSymbolicLink()) throw new Error(`Output subfolder is not a plain directory: ${current}`);
    }
  }
}
