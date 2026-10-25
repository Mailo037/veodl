// Verify npm's actual veodl tarball: the launcher, docs and versions that must
// agree with Cargo.toml and CHANGELOG.md before anything is published.
import assert from 'node:assert/strict';
import { globSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';

const archive = process.argv.slice(2).find(arg => !arg.startsWith('--')) || globSync('veodl-[0-9]*.tgz').sort().at(-1);
if (!archive) {
  console.error('FAIL: no package tarball found. Run npm pack first.');
  process.exit(1);
}
const tar = gunzipSync(readFileSync(archive));
const contents = new Map();
for (let offset = 0; offset + 512 <= tar.length;) {
  const header = tar.subarray(offset, offset + 512);
  const name = header.subarray(0, 100).toString().replace(/\0.*$/s, '');
  if (!name) break;
  const size = parseInt(header.subarray(124, 136).toString().replace(/\0/g, '').trim(), 8) || 0;
  contents.set(name, tar.subarray(offset + 512, offset + 512 + size));
  offset += 512 + Math.ceil(size / 512) * 512;
}
const names = [...contents.keys()];
for (const file of ['package/package.json', 'package/bin/veo.js', 'package/README.md', 'package/CHANGELOG.md', 'package/LICENSE', 'package/AGENTS.md', 'package/docs/AGENT_GUIDE.md']) {
  assert(names.includes(file), `Missing ${file}`);
}
assert(contents.get('package/bin/veo.js').toString().startsWith('#!/usr/bin/env node\n'), 'The launcher needs a node shebang');
assert(!names.some(name => /node_modules|\/src\/|\/target\/|\/tests?\/|\.env|\/scripts\/|\.rs$/.test(name)), 'Development files must not be published');

const pkg = JSON.parse(contents.get('package/package.json').toString());
const cargo = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync('Cargo.toml', 'utf8'))[1];
assert.equal(pkg.version, cargo, `package.json ${pkg.version} differs from Cargo.toml ${cargo}`);
assert(archive.endsWith(`veodl-${pkg.version}.tgz`), `Tarball name does not match version ${pkg.version}`);
assert(contents.get('package/CHANGELOG.md').toString().includes(`## ${pkg.version}`), `CHANGELOG.md has no "## ${pkg.version}" section`);
assert(!pkg.scripts?.preinstall && !pkg.scripts?.install && !pkg.scripts?.postinstall, 'The package must not run install scripts');
assert.deepEqual(Object.keys(pkg.dependencies || {}), [], 'The launcher has no runtime dependencies');
assert.equal(Object.keys(pkg.optionalDependencies || {}).length, 0, 'Release must be self contained');
if (!process.argv.includes('--metadata')) for (const platform of ['win32-x64','win32-arm64','darwin-x64','darwin-arm64','linux-x64','linux-arm64','android-arm64']) {
 const binary = 'package/native/' + platform + '/' + (platform.startsWith('win32') ? 'veo.exe' : 'veo');
 assert(contents.get(binary)?.length > 1000000, 'Missing native binary: ' + binary);
}
console.log('PASS: veodl ' + pkg.version + (process.argv.includes('--metadata') ? ', package metadata verified.' : ', all seven binaries included.'));
