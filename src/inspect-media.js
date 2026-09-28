import { spawn } from 'node:child_process';
import { mkdir, stat } from 'node:fs/promises';
import path from 'node:path';
import { backendCacheDirectory, exeSuffix, resolveMediaTools } from './backend.js';
import { commandOutput } from './output.js';
import { locateRunFiles, readRunArchive, RUN_ARCHIVE_ID } from './run-archive.js';

const SIGNAL_THRESHOLD_DBFS = -60;
const OUTPUT_LIMIT = 2 * 1024 * 1024;

export const INSPECT_HELP = `veo inspect <media-file> [--check-audio] [--json]
veo inspect run <id> [--check-audio] [--search <directory>] [--json]

Read local media metadata with ffprobe. --check-audio decodes every audio track
from start to end and measures its peak level with FFmpeg. A signal is detected
when the peak is above -60 dBFS; this is a level check, not a listening test.
Inspect a finished run by its persistent id. Missing media is searched by its
file fingerprint under the original output directory; --search adds another
directory after a move. Missing files are reported before media probing.
Inspection uses locally available media tools and does not contact the network.
`;

export function runMediaTool(executable, args, { signal, env } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, { shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], signal, ...(env ? { env } : {}) });
    let stdout = '', stderr = '', overflow = false;
    const append = (current, chunk) => {
      if (current.length + chunk.length > OUTPUT_LIMIT) { overflow = true; child.kill(); return current; }
      return current + chunk;
    };
    child.stdout.on('data', chunk => { stdout = append(stdout, chunk.toString()); });
    child.stderr.on('data', chunk => { stderr = append(stderr, chunk.toString()); });
    child.on('error', reject);
    child.on('close', code => {
      if (overflow) reject(new Error('Media tool output exceeded the supported size.'));
      else if (signal?.aborted) reject(new DOMException('Cancelled', 'AbortError'));
      else if (code !== 0) reject(new Error(stderr.trim().split(/\r?\n/).at(-1) || `Media tool exited with code ${code}.`));
      else resolve({ stdout, stderr });
    });
  });
}

async function localMediaTools() {
  const directory = backendCacheDirectory();
  await mkdir(directory, { recursive: true });
  const location = await resolveMediaTools({ directory, offline: true });
  return { ffmpeg: path.join(location, `ffmpeg${exeSuffix()}`), ffprobe: path.join(location, `ffprobe${exeSuffix()}`) };
}

function number(value) {
  if (value === undefined || value === null || value === '') return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : null;
}

export function parsePeak(output) {
  const match = output.match(/max_volume:\s*(-inf|-?\d+(?:\.\d+)?)\s*dB/i);
  if (!match) throw new Error('FFmpeg did not report an audio peak level.');
  return match[1].toLowerCase() === '-inf' ? null : Number(match[1]);
}

export async function inspectMedia(file, { checkAudio = false, signal, runner = runMediaTool, tools = localMediaTools } = {}) {
  const target = path.resolve(file);
  if (!(await stat(target)).isFile()) throw new Error(`Not a regular media file: ${target}`);
  const { ffprobe, ffmpeg } = await tools();
  const probe = await runner(ffprobe, ['-v', 'error', '-show_format', '-show_streams', '-of', 'json', '-i', target], { signal });
  let metadata;
  try { metadata = JSON.parse(probe.stdout); }
  catch { throw new Error(`ffprobe returned invalid metadata for ${target}.`); }
  if (!Array.isArray(metadata.streams)) throw new Error(`ffprobe found no stream metadata for ${target}.`);
  const streams = metadata.streams.map(stream => ({
    index: stream.index, type: stream.codec_type || null, codec: stream.codec_name || null,
    language: stream.tags?.language || null, width: number(stream.width), height: number(stream.height),
    channels: number(stream.channels), sampleRate: number(stream.sample_rate),
  }));
  const audioTracks = streams.filter(stream => stream.type === 'audio');
  const result = { file: target, format: metadata.format?.format_name || null,
    durationSeconds: number(metadata.format?.duration), sizeBytes: number(metadata.format?.size),
    streams, hasAudioTrack: audioTracks.length > 0, hasVideoTrack: streams.some(stream => stream.type === 'video'),
    audioCheck: null };
  if (checkAudio) {
    const tracks = [];
    for (const [audioIndex, stream] of audioTracks.entries()) {
      const report = await runner(ffmpeg, ['-hide_banner', '-nostdin', '-nostats', '-i', target,
        '-map', `0:a:${audioIndex}`, '-vn', '-sn', '-dn', '-af', 'volumedetect', '-f', 'null', '-'], { signal });
      const maxDbfs = parsePeak(report.stderr);
      tracks.push({ streamIndex: stream.index, maxDbfs, hasSignal: maxDbfs !== null && maxDbfs > SIGNAL_THRESHOLD_DBFS });
    }
    result.audioCheck = { checked: true, scope: 'full_file', thresholdDbfs: SIGNAL_THRESHOLD_DBFS,
      hasSignal: tracks.some(track => track.hasSignal), tracks };
  }
  return result;
}

/** A fast post-download container/stream check; it does not decode the full file. */
export async function verifySavedMedia(file, { audio = false, signal, inspect = inspectMedia } = {}) {
  let report;
  try { report = await inspect(file, { signal }); }
  catch (error) { throw new Error(`Verification failed for ${file}: ${error.message}`); }
  if (audio ? !report.hasAudioTrack : !report.hasVideoTrack) {
    throw new Error(`Verification failed for ${file}: no ${audio ? 'audio' : 'video'} track found.`);
  }
  return report;
}

export async function inspectRun(id, { checkAudio = false, searchRoot, signal, root, inspect = inspectMedia } = {}) {
  if (!RUN_ARCHIVE_ID.test(id)) throw new Error('Run ids have 6 letters or digits. List active runs with veo runs --json or finished attempts with veo history --json.');
  const archive = await readRunArchive(id, root);
  const located = await locateRunFiles(archive, { searchRoot, signal });
  const items = [];
  let missingFiles = 0;
  for (const item of located) {
    const files = [];
    for (const file of item.files) {
      if (!file.found) missingFiles++;
      let metadata = null;
      if (file.found && file.media) {
        // locateRunFiles verifies that this is a regular file before any probe.
        try { metadata = await inspect(file.path, { checkAudio, signal }); }
        catch (error) {
          if (error.code !== 'ENOENT') throw error;
          file.found = false; file.path = null; missingFiles++;
        }
      }
      files.push({ ...file, metadata });
    }
    items.push({ ...item, files });
  }
  return { runId: archive.id, status: archive.status, startedAt: archive.startedAt,
    finishedAt: archive.finishedAt, missingFiles, items };
}

export async function inspectMain(args = [], { stdout = process.stdout, inspect = inspectMedia, inspectRunImpl = inspectRun } = {}) {
  [args, stdout] = commandOutput(args, stdout);
  if (args.length === 1 && ['--help', '-h'].includes(args[0])) { stdout.write(INSPECT_HELP); return 0; }
  const json = args.includes('--json');
  const checkAudio = args.includes('--check-audio');
  const runMode = args[0] === 'run';
  let searchRoot;
  const positionals = [];
  for (let index = 0; index < args.length; index++) {
    const arg = args[index];
    if (['--json', '--check-audio'].includes(arg)) continue;
    if (arg === '--search' && runMode && !searchRoot) { searchRoot = args[++index]; continue; }
    positionals.push(arg);
  }
  if (runMode ? positionals.length !== 2 || positionals[0] !== 'run' || !RUN_ARCHIVE_ID.test(positionals[1]) || args.includes('--search') && (!searchRoot || searchRoot.startsWith('--'))
    : positionals.length !== 1 || positionals[0].startsWith('--')) {
    throw new Error('Usage: veo inspect <media-file> [--check-audio] [--json] | veo inspect run <id> [--check-audio] [--search <directory>] [--json]');
  }
  const controller = new AbortController();
  const cancel = () => controller.abort();
  process.once('SIGINT', cancel);
  process.once('SIGTERM', cancel);
  try {
    const result = runMode
      ? await inspectRunImpl(positionals[1], { checkAudio, searchRoot, signal: controller.signal })
      : await inspect(positionals[0], { checkAudio, signal: controller.signal });
    if (json) stdout.write(`${JSON.stringify(result)}\n`);
    else if (runMode) {
      stdout.write(`Run: ${result.runId} (${result.status})\n`);
      for (const item of result.items) for (const file of item.files) {
        stdout.write(`${file.found ? 'File' : 'Missing'}: ${file.path || file.originalPath}${file.renamed ? ` (renamed from ${file.originalPath})` : ''}\n`);
        if (file.metadata) stdout.write(`  Format: ${file.metadata.format || 'unknown'}; audio tracks: ${file.metadata.streams.filter(stream => stream.type === 'audio').length}${file.metadata.audioCheck ? `; signal: ${file.metadata.audioCheck.hasSignal ? 'detected' : 'not detected'}` : ''}\n`);
      }
    }
    else {
      stdout.write(`File: ${result.file}\n`);
      stdout.write(`Format: ${result.format || 'unknown'}; duration: ${result.durationSeconds ?? 'unknown'} s\n`);
      stdout.write(`Streams: ${result.streams.map(stream => `${stream.type || 'unknown'} ${stream.codec || 'unknown'}`).join(', ') || 'none'}\n`);
      stdout.write(`Audio tracks: ${result.streams.filter(stream => stream.type === 'audio').length}\n`);
      if (result.audioCheck) stdout.write(`Audio signal: ${result.audioCheck.hasSignal ? 'detected' : 'not detected'} (threshold ${SIGNAL_THRESHOLD_DBFS} dBFS, full file)\n`);
    }
    return runMode && result.missingFiles ? 1 : 0;
  } finally {
    process.removeListener('SIGINT', cancel);
    process.removeListener('SIGTERM', cancel);
  }
}
