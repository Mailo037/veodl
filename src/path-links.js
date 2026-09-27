import path from 'node:path';
import { statSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function supportsPathLinks(stream, env = process.env) {
  return Boolean(stream.isTTY && env.TERM !== 'dumb' &&
    (env.WT_SESSION || env.TERM_PROGRAM === 'iTerm.app' || env.TERM_PROGRAM === 'WezTerm' ||
      env.TERM_PROGRAM === 'vscode' || env.KITTY_WINDOW_ID || Number(env.VTE_VERSION) >= 5000));
}

// File links target their containing folder so clicking does not play the media.
export function folderLink(stream, target, label = target, env = process.env) {
  const safeLabel = String(label).replace(/[\x00-\x1f\x7f-\x9f]/g, ' ');
  if (!supportsPathLinks(stream, env)) return safeLabel;
  try {
    const absolute = path.resolve(target);
    const folder = statSync(absolute).isDirectory() ? absolute : path.dirname(absolute);
    const url = pathToFileURL(folder + path.sep).href;
    return `\x1b]8;;${url}\x1b\\${safeLabel}\x1b]8;;\x1b\\`;
  } catch { return safeLabel; }
}

// Recognize absolute paths in application prose before display truncation.
// Resolve the longest existing candidate so spaces and parentheses in names survive.
export function linkOutputPaths(stream, original, displayed = original, env = process.env) {
  if (!supportsPathLinks(stream, env) || /\x1b/.test(original) ||
    /^\s*(?:["{[]|Retry:|Usage:|Manual command:|Run with|Stop:|Cleanup:)/.test(original)) return displayed;
  const matches = [...original.matchAll(/(?:^|[\s("'])([A-Za-z]:[\\/]|\\\\[^\\\s]+\\|\/(?!\/))/g)];
  for (const match of matches.reverse()) {
    const start = match.index + match[0].length - match[1].length;
    const rest = original.slice(start);
    const ends = [rest.length];
    for (let i = rest.length - 1; i > 0; i--) {
      if (/[\s)"',;:.]/.test(rest[i])) ends.push(i);
    }
    let target;
    for (const end of ends) {
      const candidate = rest.slice(0, end);
      try { statSync(candidate); target = candidate; break; } catch { /* try a prose boundary */ }
    }
    if (!target) continue;
    // Truncation has already happened; keep the complete destination in the URL.
    if (displayed.slice(start, start + target.length) === target) {
      displayed = displayed.slice(0, start) + folderLink(stream, target, target, env) + displayed.slice(start + target.length);
    } else {
      const visible = displayed.slice(start);
      if (visible.endsWith('…') && target.startsWith(visible.slice(0, -1))) {
        displayed = displayed.slice(0, start) + folderLink(stream, target, visible, env);
      }
    }
  }
  return displayed;
}
