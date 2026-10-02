import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { cleanText, validateUrl } from './utils.js';

const MEDIA_URL = /\.(?:m3u8|mpd|mp4|webm|mov)(?:[?#]|$)/i;
const MEDIA_TYPE = /(?:mpegurl|dash\+xml|video\/(?:mp4|webm|quicktime))/i;
const PLAY_CONTROL = String.raw`(() => {
  const visited = new WeakSet();
  const state = globalThis[Symbol.for('veo.playControls')] ||= { visited, clicks: 0 };
  const labelMatches = node => {
    const label = [node.getAttribute('aria-label'), node.getAttribute('title'), node.getAttribute('data-testid'),
      node.getAttribute('value'), node.textContent].find(value => value && value.trim())?.trim() || '';
    return /^(?:play|watch|start|replay|abspielen|ansehen|wiedergabe)(?:\s+(?:video|movie|film|now))?$/i.test(label)
      || /(?:^|[-_\s])(?:big[-_])?play(?:[-_\s]|$)/i.test(node.className?.baseVal || node.className || '');
  };
  const documents = [document];
  for (let index = 0; index < documents.length && index < 16; index++) {
    for (const frame of documents[index].querySelectorAll('iframe')) {
      try { if (frame.contentDocument && !documents.includes(frame.contentDocument)) documents.push(frame.contentDocument); } catch {}
    }
  }
  for (const doc of documents) {
    const controls = doc.querySelectorAll('button,[role="button"],[aria-label],[title],[data-testid],input[type="button"],input[type="image"],[class*="play"],[class*="Play"]');
    for (const node of controls) {
      if (state.clicks >= 8) return false;
      if (state.visited.has(node) || node.disabled || node.getAttribute('aria-disabled') === 'true' || !labelMatches(node)) continue;
      const style = doc.defaultView.getComputedStyle(node);
      if (style.display === 'none' || style.visibility === 'hidden' || style.pointerEvents === 'none' || !node.getClientRects().length) continue;
      state.visited.add(node);
      state.clicks++;
      node.click();
      return true;
    }
    for (const video of doc.querySelectorAll('video')) {
      if (state.visited.has(video) || !video.paused) continue;
      state.visited.add(video);
      video.play().catch(() => {});
      return true;
    }
  }
  return false;
})()`;

function abortable(promise, signal) {
  if (!signal) return promise;
  if (signal.aborted) {
    Promise.resolve(promise).catch(() => {});
    throw signal.reason;
  }
  return new Promise((resolve, reject) => {
    const onAbort = () => reject(signal.reason);
    signal.addEventListener('abort', onAbort, { once: true });
    Promise.resolve(promise).then(resolve, reject).finally(() => signal.removeEventListener('abort', onAbort));
  });
}

export function parseSourceTimeout(value) {
  const match = /^(\d+)(ms|s|m)?$/i.exec(String(value || '').trim());
  if (!match) throw new Error('--timeout expects seconds, e.g. 30, 30s or 2m.');
  const milliseconds = Number(match[1]) * ({ ms: 1, s: 1000, m: 60000 }[match[2]?.toLowerCase() || 's']);
  if (!Number.isSafeInteger(milliseconds) || milliseconds < 5000 || milliseconds > 600000) {
    throw new Error('--timeout must be between 5 seconds and 10 minutes.');
  }
  return milliseconds;
}

export function shouldOfferSourceDiscovery(error) {
  if (error?.name === 'AbortError') return false;
  return !/(?:HTTP (?:Error )?(?:401|403|404|429|5\d\d)|\b(?:DRM|live stream|rate limit|timed out|timeout|connection refused|login required|sign in|private video|geo.?block)\b|This URL is a collection)/i
    .test(String(error?.message || error));
}

export function browserCandidates() {
  const custom = process.env.VEO_BROWSER_PATH;
  if (custom) return [custom];
  if (process.platform === 'win32') {
    const roots = [process.env.PROGRAMFILES, process.env['PROGRAMFILES(X86)'], process.env.LOCALAPPDATA].filter(Boolean);
    return roots.flatMap(root => [
      path.join(root, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      path.join(root, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
      path.join(root, 'BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'),
    ]);
  }
  if (process.platform === 'darwin') return [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
  ];
  return ['chromium', 'chromium-browser', 'google-chrome', 'microsoft-edge', 'brave-browser'];
}

export function mediaUrlsFromText(body) {
  if (typeof body !== 'string' || body.length > 524288) return [];
  const normalized = body.replaceAll('\\/', '/').replaceAll('\\u0026', '&').replaceAll('&amp;', '&');
  const urls = normalized.match(/https?:\/\/[^\s"'<>\\]+/gi) || [];
  return [...new Set(urls.map(value => value.replace(/[),;]+$/, '')).filter(value => MEDIA_URL.test(value)).map(value => {
    try { return validateUrl(value); } catch { return null; }
  }).filter(Boolean))];
}

export function summarizeSource(metadata, url, index, pageUrl) {
  const formats = (metadata.formats || []).filter(format => !format.has_drm);
  const heights = [...new Set(formats.map(format => format.height).filter(Number.isFinite))].sort((a, b) => b - a);
  const bytes = metadata.filesize || metadata.filesize_approx || null;
  const bitrate = Number.isFinite(metadata.tbr) ? metadata.tbr : Math.max(0, ...formats.map(format => format.tbr).filter(Number.isFinite));
  return {
    index, title: cleanText(metadata.title || metadata.id || 'Video'),
    source: new URL(url).hostname, type: /\.m3u8(?:[?#]|$)/i.test(url) ? 'HLS' : /\.mpd(?:[?#]|$)/i.test(url) ? 'DASH' : 'Video',
    quality: heights.length ? heights.map(height => `${height}p`) : [],
    estimatedMiB: bytes ? Math.round(bytes / 1048576 * 10) / 10 : null,
    bitrateMbps: bitrate > 0 ? Math.round(bitrate / 100) / 10 : null,
    pageUrl,
  };
}

function createCdp(socket) {
  let nextId = 0;
  const pending = new Map();
  const listeners = new Set();
  socket.addEventListener('message', event => {
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message.id) {
      const entry = pending.get(message.id);
      if (!entry) return;
      pending.delete(message.id);
      if (message.error) entry.reject(new Error(message.error.message));
      else entry.resolve(message.result || {});
    } else for (const listener of listeners) listener(message);
  });
  socket.addEventListener('close', () => {
    for (const entry of pending.values()) entry.reject(new Error('Browser connection closed.'));
    pending.clear();
  });
  return {
    on(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    send(method, params = {}, sessionId) {
      const id = ++nextId;
      const promise = new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
      socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      return promise;
    },
  };
}

async function findDebuggerPort(directory, child, signal) {
  // Cold Chromium startup on Windows runners can exceed eight seconds.
  // The caller's scan deadline and cancellation still bound this wait.
  for (let attempt = 0; attempt < 300; attempt++) {
    signal?.throwIfAborted();
    if (child.exitCode !== null) throw new Error('The browser exited before source discovery started.');
    const text = await readFile(path.join(directory, 'DevToolsActivePort'), 'utf8').catch(() => null);
    if (text) return Number(text.split(/\r?\n/)[0]);
    await delay(100, undefined, { signal });
  }
  throw new Error('The browser did not start its debugging connection.');
}

/** Observe media requests in a temporary browser profile; no browser cookies are copied. */
export async function captureBrowserMedia(pageUrl, { signal, browserPaths = browserCandidates(), observeMs = 8000, deepScan = false } = {}) {
  const directory = await mkdtemp(path.join(tmpdir(), 'veo-discover-'));
  let child, socket;
  const urls = new Set();
  try {
    let started = false;
    for (const executable of browserPaths) {
      try {
        child = spawn(executable, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${directory}`,
          '--no-first-run', '--no-default-browser-check', '--autoplay-policy=no-user-gesture-required', 'about:blank'],
        { stdio: 'ignore', windowsHide: true });
        await new Promise((resolve, reject) => {
          child.once('spawn', resolve);
          child.once('error', reject);
        });
        started = true;
        break;
      } catch { child = null; }
    }
    if (!started) throw new Error('No Chromium-based browser found. Set VEO_BROWSER_PATH to Chrome, Edge or Chromium.');
    const port = await findDebuggerPort(directory, child, signal);
    const endpoint = await fetch(`http://127.0.0.1:${port}/json/version`, { signal }).then(response => response.json());
    socket = new WebSocket(endpoint.webSocketDebuggerUrl);
    await abortable(new Promise((resolve, reject) => {
      socket.addEventListener('open', resolve, { once: true });
      socket.addEventListener('error', reject, { once: true });
    }), signal);
    const cdp = createCdp(socket);
    const responses = new Map();
    const bodyReads = [];
    const sessions = new Set();
    const collect = (value, mime = '') => {
      if (!MEDIA_URL.test(value) && !MEDIA_TYPE.test(mime)) return;
      try { urls.add(validateUrl(value)); } catch {}
    };
    cdp.on(message => {
      if (message.method === 'Target.attachedToTarget') {
        const attached = message.params.sessionId;
        sessions.add(attached);
        cdp.send('Network.enable', {}, attached).catch(() => {});
        return;
      }
      if (message.method === 'Target.detachedFromTarget') sessions.delete(message.params.sessionId);
      if (message.method === 'Network.requestWillBeSent') collect(message.params.request?.url);
      if (message.method === 'Network.responseReceived') {
        const response = message.params.response;
        collect(response.url, response.mimeType);
        if (/(?:json|javascript|text\/plain)/i.test(response.mimeType || '')) {
          responses.set(`${message.sessionId || ''}:${message.params.requestId}`, { sessionId: message.sessionId, requestId: message.params.requestId });
        }
      }
      if (message.method === 'Network.loadingFinished') {
        const key = `${message.sessionId || ''}:${message.params.requestId}`;
        const response = responses.get(key);
        if (!response || message.params.encodedDataLength > 524288) return;
        responses.delete(key);
        if (!deepScan && bodyReads.length >= 100) return;
        bodyReads.push(cdp.send('Network.getResponseBody', { requestId: response.requestId }, response.sessionId)
          .then(result => { if (!result.base64Encoded) for (const url of mediaUrlsFromText(result.body)) urls.add(url); })
          .catch(() => {}));
      }
    });
    await abortable(cdp.send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }), signal);
    const target = await abortable(cdp.send('Target.createTarget', { url: 'about:blank' }), signal);
    const attached = await abortable(cdp.send('Target.attachToTarget', { targetId: target.targetId, flatten: true }), signal);
    sessions.add(attached.sessionId);
    await abortable(cdp.send('Network.enable', {}, attached.sessionId), signal);
    await abortable(cdp.send('Page.enable', {}, attached.sessionId), signal);
    await abortable(cdp.send('Page.navigate', { url: pageUrl }, attached.sessionId), signal);
    const observeUntil = Date.now() + observeMs;
    while (Date.now() < observeUntil) {
      signal?.throwIfAborted();
      await Promise.allSettled([...sessions].map(sessionId => abortable(cdp.send('Runtime.evaluate', {
        expression: PLAY_CONTROL, returnByValue: true,
      }, sessionId), signal)));
      if (!deepScan && urls.size > 0) break;
      const remaining = observeUntil - Date.now();
      if (remaining > 0) await delay(Math.min(300, remaining), undefined, { signal });
    }
    await Promise.race([Promise.allSettled(bodyReads), delay(1000, undefined, { signal }).catch(() => {})]);
    return [...urls];
  } catch (error) {
    if (signal?.aborted && ['AbortError', 'TimeoutError'].includes(error?.name)) return [...urls];
    throw error;
  } finally {
    socket?.close();
    child?.kill();
    await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(() => {});
  }
}

/** Verify candidates with yt-dlp before offering them to the user. */
export async function discoverSources(pageUrl, { signal, capture = captureBrowserMedia, inspect, limit = 30,
  deepScan = false, timeoutMs = deepScan ? 120000 : 45000 } = {}) {
  signal?.throwIfAborted();
  const timeout = new AbortController();
  const timer = setTimeout(() => timeout.abort(new DOMException('Source scan timed out.', 'TimeoutError')), timeoutMs);
  const scanSignal = signal ? AbortSignal.any([signal, timeout.signal]) : timeout.signal;
  const sources = [];
  const seen = new Set();
  const formatReferences = new Map();
  const mediaKey = value => {
    try { const address = new URL(value); return `${address.origin}${address.pathname}`; }
    catch { return null; }
  };
  try {
    const observeMs = deepScan ? Math.min(30000, Math.floor(timeoutMs * 0.6)) : Math.min(8000, Math.floor(timeoutMs * 0.5));
    let urls;
    try { urls = await capture(pageUrl, { signal: scanSignal, observeMs, deepScan }); }
    catch (error) { if (!timeout.signal.aborted) throw error; urls = []; }
    signal?.throwIfAborted();
    for (const url of deepScan ? urls : urls.slice(0, limit)) {
      if (timeout.signal.aborted) break;
      signal?.throwIfAborted();
      try {
        const metadata = await abortable(inspect(url, pageUrl, { signal: scanSignal }), scanSignal);
        if (metadata.has_drm || metadata.is_live || metadata.formats?.length && metadata.formats.every(format => format.has_drm)
          || !metadata.formats?.length && !metadata.url) continue;
        const address = new URL(url);
        const key = `${address.origin}${address.pathname}:${metadata.id || ''}`;
        if (seen.has(key)) continue;
        seen.add(key);
        sources.push({ ...summarizeSource(metadata, url, sources.length + 1, pageUrl), url });
        formatReferences.set(url, new Set((metadata.formats || []).map(format => mediaKey(format.url)).filter(Boolean)));
      } catch { /* A request alone is not proof of downloadable media. */ }
    }
    signal?.throwIfAborted();
    // A master playlist and its variant streams describe the same video.
    const distinct = sources.filter(source => !sources.some(other => other !== source
      && formatReferences.get(other.url)?.has(mediaKey(source.url))));
    distinct.forEach((source, index) => { source.index = index + 1; });
    Object.defineProperty(distinct, 'timedOut', { value: timeout.signal.aborted });
    return distinct;
  } finally {
    clearTimeout(timer);
  }
}

export function formatSource(source) {
  const size = source.estimatedMiB === null ? 'Size unknown' : `about ${source.estimatedMiB} MiB`;
  const bitrate = source.bitrateMbps === null ? '' : ` | ${source.bitrateMbps} Mbit/s`;
  return `${source.index}. ${source.title} | ${source.source} | ${source.type} | ${source.quality.join(', ') || 'Quality unknown'} | ${size}${bitrate}`;
}
