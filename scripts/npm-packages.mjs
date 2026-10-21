// Generates the npm packages from Cargo.toml's version:
//   node scripts/npm-packages.mjs            write package.json files
//   node scripts/npm-packages.mjs --check    fail when they are out of date
// Each platform package carries one prebuilt binary in bin/; the release
// workflow copies the binaries in before publishing.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const version = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(path.join(root, 'Cargo.toml'), 'utf8'))[1];

// npm platform/arch → Rust target triple built by the release workflow.
export const PLATFORMS = {
  'win32-x64': { os: 'win32', cpu: 'x64', target: 'x86_64-pc-windows-msvc' },
  'win32-arm64': { os: 'win32', cpu: 'arm64', target: 'aarch64-pc-windows-msvc' },
  'darwin-x64': { os: 'darwin', cpu: 'x64', target: 'x86_64-apple-darwin' },
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', target: 'aarch64-apple-darwin' },
  'linux-x64': { os: 'linux', cpu: 'x64', target: 'x86_64-unknown-linux-musl' },
  'linux-arm64': { os: 'linux', cpu: 'arm64', target: 'aarch64-unknown-linux-musl' },
  'android-arm64': { os: 'android', cpu: 'arm64', target: 'aarch64-linux-android' },
};

const json = value => `${JSON.stringify(value, null, 2)}\n`;
const files = new Map();

const main = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8'));
main.version = version;
main.optionalDependencies = Object.fromEntries(Object.keys(PLATFORMS).map(name => [`veodl-${name}`, version]));
files.set(path.join(root, 'package.json'), json(main));

for (const [name, platform] of Object.entries(PLATFORMS)) {
  files.set(path.join(root, 'npm', name, 'package.json'), json({
    name: `veodl-${name}`,
    version,
    description: `The veo binary for ${platform.os} ${platform.cpu} (${platform.target}). Install veodl instead.`,
    license: 'MIT',
    repository: main.repository,
    homepage: main.homepage,
    os: [platform.os],
    cpu: [platform.cpu],
    files: ['bin/'],
    preferUnplugged: true,
  }));
}

const check = process.argv.includes('--check');
let stale = 0;
for (const [file, content] of files) {
  let current = null;
  try { current = readFileSync(file, 'utf8'); } catch { /* Missing counts as stale. */ }
  if (current === content) continue;
  stale++;
  if (check) console.error(`Out of date: ${path.relative(root, file)} (run node scripts/npm-packages.mjs)`);
  else {
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
    console.log(`Wrote ${path.relative(root, file)}`);
  }
}
if (check && stale) process.exit(1);
if (check) console.log(`npm packages match version ${version}.`);
