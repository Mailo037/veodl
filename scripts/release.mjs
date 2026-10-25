// Publish the verified, prebuilt tarballs locally using the user's npm login.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
process.chdir(fileURLToPath(new URL('../', import.meta.url)));
try {
  if (process.argv.slice(2).some(arg => arg !== '--check')) throw new Error('Usage: npm run release [-- --check]');
  const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
  const manifest = JSON.parse(readFileSync('release/npm/manifest.json', 'utf8'));
  if (manifest.version !== pkg.version) throw new Error('Artifact version differs from package.json.');
  const expected = [...Object.keys(pkg.optionalDependencies), pkg.name].map(name => `${name}-${pkg.version}.tgz`);
  if (JSON.stringify(manifest.files.map(entry => entry.name)) !== JSON.stringify(expected)) throw new Error('Artifact must contain all seven platforms followed by veodl.');
  for (const entry of manifest.files) {
    const digest = createHash('sha256').update(readFileSync(`release/npm/${entry.name}`)).digest('hex');
    if (digest !== entry.sha256) throw new Error(`Checksum mismatch: ${entry.name}`);
  }
  console.log(`Verified ${expected.length} npm tarballs for ${pkg.version}.`);
  if (!process.argv.includes('--check')) {
    if (!process.env.npm_execpath) throw new Error('Run this command through npm run release.');
    for (const name of expected) {
      console.log(`Publishing ${name}`);
      execFileSync(process.execPath, [process.env.npm_execpath, 'publish', `release/npm/${name}`, '--access', 'public', '--tag', 'latest'], { stdio: 'inherit' });
    }
  }
} catch (error) {
  console.error(`Release stopped: ${error.message}`);
  process.exitCode = 1;
}
