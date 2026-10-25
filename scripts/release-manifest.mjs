import { readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
const files = [...Object.keys(pkg.optionalDependencies), pkg.name].map(name => {
  const file = `${name}-${pkg.version}.tgz`;
  return { name: file, sha256: createHash('sha256').update(readFileSync(`release/npm/${file}`)).digest('hex') };
});
writeFileSync('release/npm/manifest.json', `${JSON.stringify({ version: pkg.version, commit: process.env.GITHUB_SHA || null, files }, null, 2)}\n`);
