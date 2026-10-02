// End-to-end CLI test with real local media and a controlled desktop-opener
// replacement. Does not launch a GUI or depend on installed file associations.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { exeSuffix, resolveMediaTools } from '../src/backend.js';

const cli = fileURLToPath(new URL('../bin/veo.js', import.meta.url));
const root = await mkdtemp(path.join(os.tmpdir(), 'veo-open-'));

// The detached opener inherits the CLI's working directory, so Windows keeps
// that directory busy for a moment after the assertions have already passed.
async function cleanup(target) {
  for (let attempt = 0; attempt < 15; attempt++) {
    try {
      await rm(target, { recursive: true, force: true });
      return;
    } catch (error) {
      if (!['EBUSY', 'EPERM', 'ENOTEMPTY'].includes(error.code)) throw error;
      await delay(200);
    }
  }
  console.warn(`Note: ${target} could not be removed because a detached opener still holds it.`);
}
function run(command, args, env = process.env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: 'inherit', shell: false, env });
    child.once('error', reject);
    child.once('close', resolve);
  });
}
let server;
try {
  const toolCache = path.join(root, 'smoke-tools');
  await mkdir(toolCache);
  const mediaDirectory = await resolveMediaTools({ directory: toolCache });
  const ffmpeg = path.join(mediaDirectory, `ffmpeg${exeSuffix()}`);
  const source = path.join(root, 'fixture.mp4');
  assert.equal(await run(ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=160x90:r=10', '-t', '1', '-c:v', 'libx264', source]), 0);
  const media = await readFile(source);
  server = createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'video/mp4', 'Content-Length': media.length });
    res.end(req.method === 'HEAD' ? undefined : media);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const hook = path.join(root, 'opener-hook.cjs');
  const log = path.join(root, 'opened.json');
  await writeFile(hook, `
const cp = require('node:child_process');
const fs = require('node:fs');
const original = cp.spawn;
cp.spawn = function(command, args, options) {
  if (['explorer.exe', 'open', 'xdg-open', 'termux-open'].includes(command)) {
    // Verify the final file already exists when the real CLI asks to open it.
    const size = fs.statSync(args[0]).size;
    const child = original(process.execPath, ['-e', 'process.exit(0)'], options);
    child.once('spawn', () => fs.writeFileSync(process.env.VEO_OPEN_TEST_LOG,
      JSON.stringify({ command, args, options, size, pid: child.pid })));
    return child;
  }
  return original(command, args, options);
};
require('node:module').syncBuiltinESMExports();
`);
  const url = `http://127.0.0.1:${server.address().port}/video.mp4`;
  const env = { ...process.env, VEO_OPEN_TEST_LOG: log,
    VEO_CONFIG: path.join(root, 'missing-config.json'), VEO_NO_UPDATE_CHECK: '1',
    LOCALAPPDATA: root, XDG_CACHE_HOME: root };
  console.log('Testing CLI: veo <local-video> -r "My Video" --open');
  assert.equal(await run(process.execPath, ['--require', hook, cli, url, '-r', 'My Video', '--open'], env), 0);
  const launch = JSON.parse(await readFile(log, 'utf8'));
  assert.equal(launch.command, process.platform === 'win32' ? 'explorer.exe' : process.platform === 'darwin' ? 'open' : process.platform === 'android' ? 'termux-open' : 'xdg-open');
  assert.deepEqual(launch.args, [path.join(root, 'My Video.mp4')]);
  assert.equal(launch.options.shell, false);
  assert.equal(launch.options.detached, true);
  assert.equal(launch.size, media.length);
  assert(launch.pid > 0);
  console.log(`PASS: CLI requested ${launch.command} ${JSON.stringify(launch.args)} after saving; replacement subprocess launched (PID ${launch.pid}).`);
} finally {
  if (server) await new Promise(resolve => server.close(resolve));
  await cleanup(root);
}
