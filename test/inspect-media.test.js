import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { INSPECT_HELP, inspectMain, inspectMedia, parsePeak, runMediaTool } from '../src/inspect-media.js';
import { exeSuffix, resolveMediaTools } from '../src/backend.js';

const sink = () => { let value = ''; return { stdout: { write: chunk => { value += chunk; } }, read: () => value }; };

test('audio inspection distinguishes no track, digital silence and a signal', async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'veo-inspect-'));
  try {
    const mediaDirectory = await resolveMediaTools({ offline: true });
    const ffmpeg = path.join(mediaDirectory, `ffmpeg${exeSuffix()}`);
    const ffprobe = path.join(mediaDirectory, `ffprobe${exeSuffix()}`);
    const tools = async () => ({ ffmpeg, ffprobe });
    const sound = path.join(directory, 'sound.wav');
    const silence = path.join(directory, 'silence.wav');
    const video = path.join(directory, 'video.mp4');
    const twoTracks = path.join(directory, 'two-tracks.mkv');
    await runMediaTool(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=0.25', '-c:a', 'pcm_s16le', sound]);
    await runMediaTool(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'anullsrc=r=8000:cl=mono:d=0.25', '-c:a', 'pcm_s16le', silence]);
    await runMediaTool(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=16x16:d=0.25', '-c:v', 'mpeg4', video]);
    await runMediaTool(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-i', silence, '-i', sound,
      '-map', '0:a:0', '-map', '1:a:0', '-c:a', 'pcm_s16le', twoTracks]);

    const found = await inspectMedia(sound, { checkAudio: true, tools });
    assert.equal(found.hasAudioTrack, true);
    assert.equal(found.hasVideoTrack, false);
    assert.equal(found.audioCheck.hasSignal, true);
    assert.ok(found.audioCheck.tracks[0].maxDbfs > -60);
    const cli = await runMediaTool(process.execPath, [path.resolve('bin/veo.js'), 'inspect', sound, '--check-audio', '--json']);
    assert.equal(JSON.parse(cli.stdout).audioCheck.hasSignal, true);

    const quiet = await inspectMedia(silence, { checkAudio: true, tools });
    assert.equal(quiet.hasAudioTrack, true);
    assert.equal(quiet.audioCheck.hasSignal, false);
    assert.ok(quiet.audioCheck.tracks[0].maxDbfs === null || quiet.audioCheck.tracks[0].maxDbfs <= -60);

    const noTrack = await inspectMedia(video, { checkAudio: true, tools });
    assert.equal(noTrack.hasAudioTrack, false);
    assert.equal(noTrack.hasVideoTrack, true);
    assert.deepEqual(noTrack.audioCheck.tracks, []);
    assert.equal(noTrack.audioCheck.hasSignal, false);

    const mixed = await inspectMedia(twoTracks, { checkAudio: true, tools });
    assert.equal(mixed.audioCheck.tracks.length, 2);
    assert.deepEqual(mixed.audioCheck.tracks.map(track => track.hasSignal), [false, true]);
    assert.equal(mixed.audioCheck.hasSignal, true);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test('inspect help, JSON output and peak parser keep uncertain results explicit', async () => {
  const help = sink();
  assert.equal(await inspectMain(['--help'], { stdout: help.stdout }), 0);
  assert.equal(help.read(), INSPECT_HELP);
  const json = sink();
  assert.equal(await inspectMain(['example.mp4', '--check-audio', '--json'], { stdout: json.stdout,
    inspect: async (_, options) => {
      assert.equal(options.checkAudio, true);
      return { file: 'example.mp4', streams: [], hasAudioTrack: false, audioCheck: { checked: true, hasSignal: false } };
    } }), 0);
  assert.equal(JSON.parse(json.read()).audioCheck.hasSignal, false);
  assert.equal(parsePeak('max_volume: -inf dB'), null);
  assert.equal(parsePeak('max_volume: -18.2 dB'), -18.2);
  assert.throws(() => parsePeak('no peak'), /did not report/);
  await assert.rejects(inspectMain(['--bad'], { stdout: sink().stdout }), /Usage: veo inspect/);
});
