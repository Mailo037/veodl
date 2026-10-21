#!/usr/bin/env node
// Starts the native veo binary that npm installed from the matching platform
// package (veodl-<platform>-<arch>). Exit codes and signals pass through.
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

const require = createRequire(import.meta.url);
const target = `${process.platform}-${process.arch}`;
const executable = process.platform === 'win32' ? 'veo.exe' : 'veo';

function binaryPath() {
  if (process.env.VEO_BINARY) return process.env.VEO_BINARY;
  // Windows on ARM can run the x64 build through emulation.
  const candidates = [target, ...(target === 'win32-arm64' ? ['win32-x64'] : [])];
  for (const candidate of candidates) {
    try {
      const file = path.join(path.dirname(require.resolve(`veodl-${candidate}/package.json`)), 'bin', executable);
      if (existsSync(file)) return file;
    } catch { /* Try the next candidate. */ }
  }
  return null;
}

const binary = binaryPath();
if (!binary) {
  process.stderr.write(`veo: no prebuilt binary for ${target} is installed.\n`
    + 'Reinstall with optional dependencies enabled: npm install -g veodl\n'
    + 'Or build from source: cargo install --git https://github.com/Mailo037/veodl\n');
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit', windowsHide: false });
// The terminal already delivers Ctrl+C to veo itself; keep this launcher alive
// until veo has cleaned up, and forward termination requests sent only to it.
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => { if (signal !== 'SIGINT') child.kill(signal); });
}
child.on('error', error => {
  process.stderr.write(`veo: could not start ${binary}: ${error.message}\n`);
  process.exit(1);
});
child.on('close', (code, signal) => {
  if (signal) {
    process.removeAllListeners(signal);
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
