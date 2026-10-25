import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
process.chdir(fileURLToPath(new URL('../', import.meta.url)));
try {
 const { version } = JSON.parse(readFileSync('package.json', 'utf8'));
 const archive = 'veodl-' + version + '.tgz';
 execFileSync(process.execPath, ['scripts/check-package.js', archive], { stdio: 'inherit' });
 if (!process.argv.includes('--check')) {
  if (!process.env.npm_execpath) throw new Error('Use npm run release');
  execFileSync(process.execPath, [process.env.npm_execpath, 'publish', archive, '--access', 'public', '--tag', 'latest'], { stdio: 'inherit' });
 }
} catch (error) { console.error(error.message); process.exitCode = 1; }
