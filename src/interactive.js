import { outputStream, formatOutput } from './output.js';
import { createInterface } from 'node:readline/promises';
import { availableHeights, cleanText, validateUrl } from './utils.js';
import { applyProfile, selectedProfileName } from './config.js';
import { fetchMetadata, prepareBackend, runBackend } from './downloader.js';
import { describeEstimate, selectedEntries, sizeEstimate, validateItems } from './playlist.js';

export async function interactiveArgs(config, { signal, input = process.stdin, output = process.stderr, ask, inspect, discover } = {}) {
  const terminalOutput = output;
  output = outputStream(output);
  const rl = ask ? null : createInterface({ input, output: terminalOutput });
  const controller = new AbortController();
  signal = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
  rl?.on('SIGINT', () => controller.abort());
  const question = ask || (prompt => rl.question(formatOutput(prompt, terminalOutput), { signal }));
  const choose = async (prompt, allowed, fallback) => {
    while (true) {
      const answer = (await question(prompt)).trim() || fallback;
      if (allowed.includes(answer)) return answer;
      output.write(`Choose: ${allowed.join(', ')}\n`);
    }
  };
  try {
    const args = [];
    const profiles = Object.keys(config.profiles || {});
    let defaults = applyProfile(config);
    if (profiles.length) {
      const selected = selectedProfileName(config);
      const choices = selected ? profiles : [...profiles, 'none'];
      const fallback = selected || 'none';
      const profile = await choose(`Profile (${choices.join(', ')}) [${fallback}]: `, choices, fallback);
      if (profile !== 'none') { args.push('--profile', profile); defaults = applyProfile(config, profile); }
    }
    const url = validateUrl((await question('Video or playlist URL: ')).trim());
    args.push(url);
    if (defaults.listSources) {
      output.write('Listing page sources…\n');
      args.push('--list-sources');
      return args;
    }
    const type = await choose(`Download (video/audio) [${defaults.audio ? 'audio' : 'video'}]: `, ['video', 'audio'], defaults.audio ? 'audio' : 'video');
    args.push(type === 'audio' ? '--audio' : '--no-audio');
    output.write('Reading available media…\n');
    // Inspect the collection first so ordinary video links do not need a playlist prompt.
    const inspectionOptions = { ...defaults, url, playlist: true };
    const backend = inspect ? null : await prepareBackend(inspectionOptions, { signal });
    let metadata;
    try {
      metadata = inspect ? await inspect(inspectionOptions) : await fetchMetadata(inspectionOptions, { signal, backend, runner: runBackend });
      if (Array.isArray(metadata.formats) && !metadata.formats.length && !metadata.entries?.length && !metadata.url) {
        throw new Error('No downloadable video formats found.');
      }
    } catch (originalError) {
      const { discoverSources, formatSource, parseSourceTimeout, shouldOfferSourceDiscovery } = await import('./source-discovery.js');
      if (!shouldOfferSourceDiscovery(originalError)) throw originalError;
      if (defaults.autoListSources === false) throw originalError;
      const inspectSource = (mediaUrl, _page, { signal: scanSignal = signal } = {}) => inspect
        ? inspect({ ...inspectionOptions, playlist: false, mediaUrl }, scanSignal)
        : fetchMetadata({ ...inspectionOptions, playlist: false, mediaUrl }, { signal: scanSignal, backend, runner: runBackend });
      const sources = await (discover || discoverSources)(url, { signal, inspect: inspectSource,
        deepScan: defaults.deepScan, timeoutMs: defaults.timeout ? parseSourceTimeout(defaults.timeout) : undefined }).catch(() => []);
      if (!sources.length) throw originalError;
      let selection = sources[0];
      if (sources.length > 1) {
        for (const source of sources) output.write(`${formatSource(source)}\n`);
        selection = null;
        while (!selection) {
          const answer = (await question('Source number: ')).trim();
          const index = Number(answer);
          if (/^[1-9]\d*$/.test(answer) && sources[index - 1]) selection = sources[index - 1];
          else output.write(`Choose a number from 1 to ${sources.length}.\n`);
        }
      } else output.write(`Found video source: ${formatSource(selection)}\n`);
      args.push('--source', String(selection.index));
      metadata = await inspectSource(selection.url);
    }
    const isCollection = metadata._type === 'playlist' || Boolean(metadata.entries);
    const collection = isCollection
      ? await choose(`Download the playlist? (y/n) [${defaults.playlist === false ? 'n' : 'y'}]: `,
        ['y', 'n'], defaults.playlist === false ? 'n' : 'y')
      : 'n';
    args.push(collection === 'y' ? '--playlist' : '--no-playlist');
    output.write(`${cleanText(metadata.title || metadata.id || 'Media')}\n`);
    if (collection === 'y' && metadata.entries) {
      for (const { entry, index } of selectedEntries(metadata)) output.write(`${index}. ${cleanText(entry?.title || 'Unavailable entry')}\n`);
      let selection;
      while (true) {
        const preferred = defaults.playlistItems || '';
        selection = (await question(`Entries (e.g. 1,3-5; Enter = ${preferred || 'all'}): `)).trim() || preferred;
        try { if (selection) validateItems(selection); selectedEntries(metadata, selection); break; }
        catch (error) { output.write(`${error.message}\n`); }
      }
      if (selection) args.push('--playlist-items', selection);
      output.write(`${describeEstimate(sizeEstimate(selectedEntries(metadata, selection)))}\n`);
    }
    if (type === 'video') {
      const heights = availableHeights(metadata.formats).sort((a, b) => b - a).map(height => `${height}p`);
      const choices = ['best', ...heights];
      if (!heights.length) output.write('Resolution metadata unavailable; using best. Set a quality cap with -q in command mode.\n');
      const fallback = choices.includes(defaults.quality) ? defaults.quality : 'best';
      args.push('-q', await choose(`Quality (${choices.join(', ')}) [${fallback}]: `, choices, fallback));
    }
    // Switching the media type must not leave an incompatible format from a profile.
    if (type === 'video' && ['mp3', 'm4a', 'aac', 'opus', 'flac', 'wav'].includes(defaults.format)) args.push('--format', 'mp4');
    if (type === 'audio' && ['mp4', 'mkv', 'webm', 'mov'].includes(defaults.format)) args.push('--format', 'mp3');
    const directory = (await question(`Output directory [${defaults.output || process.cwd()}]: `)).trim() || defaults.output || process.cwd();
    args.push('-o', directory);
    if (defaults.incognito) {
      args.push('--no-resume', '--no-skip-existing');
    } else {
      args.push('--resume');
      const skipDefault = defaults.skipExisting === false ? 'n' : 'y';
      const skip = await choose(`Skip previously downloaded videos? (y/n) [${skipDefault}]: `, ['y', 'n'], skipDefault);
      args.push(skip === 'y' ? '--skip-existing' : '--no-skip-existing');
    }
    output.write(`Ready: ${type}, ${directory}\n`);
    if (await choose('Start download? (y/n) [y]: ', ['y', 'n'], 'y') === 'n') return null;
    return args;
  } finally { rl?.close(); }
}
