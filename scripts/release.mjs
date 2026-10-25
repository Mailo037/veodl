// Publish through the platform-build workflow, never just the npm launcher.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('../', import.meta.url)));
const run = (command, args) => execFileSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }).trim();
try {
  if (process.argv.slice(2).some(arg => arg !== '--check')) throw new Error('Usage: npm run release [-- --check]');
  run(process.execPath, ['scripts/npm-packages.mjs', '--check']);
  const { version } = JSON.parse(readFileSync('package.json', 'utf8'));
  const tag = `v${version}`;
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('A stable release version is required.');
  if (!readFileSync('CHANGELOG.md', 'utf8').split(/\r?\n/).includes(`## ${version}`)) throw new Error('Missing changelog entry.');
  if (run('git', ['tag', '--list', tag])) throw new Error(`${tag} already exists locally.`);
  if (run('git', ['ls-remote', '--tags', 'origin', `refs/tags/${tag}`])) throw new Error(`${tag} already exists on origin.`);
  const dirty = run('git', ['status', '--porcelain']);
  console.log(`Release ${tag}: GitHub Actions builds all seven platforms and publishes npm packages.`);
  if (process.argv.includes('--check')) {
    console.log(dirty ? 'Preflight OK. Commit the preparation changes before releasing.' : 'Preflight OK. Working tree is clean.');
    process.exit(0);
  }
  if (dirty) throw new Error('Commit all intended release changes before running npm run release.');
  run('git', ['tag', '-a', tag, '-m', `Release ${tag}`]);
  console.log(run('git', ['push', 'origin', `refs/tags/${tag}`]));
  console.log('Release workflow triggered. Check GitHub Actions for build and npm publication results.');
} catch (error) {
  console.error(`Release stopped: ${error.message}`);
  process.exitCode = 1;
}
