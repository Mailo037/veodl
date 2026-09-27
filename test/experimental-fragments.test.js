import test from 'node:test';
import assert from 'node:assert/strict';
import { parseCli } from '../src/cli.js';
import { parseConfigText } from '../src/config.js';
import { confirmExperimentalFragments } from '../src/experimental-fragments.js';

const url = 'https://example.com/video.mp4';

test('fragment bounds apply to CLI, profiles and explicit opt-in', () => {
  for (const n of [1, 16, 17, 64]) assert.equal(parseCli([url, '-N', String(n)]).concurrentFragments, n);
  for (const n of ['0', '65', '1.5', 'NaN', 'Infinity']) {
    assert.throws(() => parseCli([url, '-N', n, '--experimental-fragments']), /between 1 and 64/);
  }
  const { config } = parseConfigText('{"profiles":{"fast":{"concurrentFragments":32,"experimentalFragments":true}}}');
  const options = parseCli([url, '--profile', 'fast'], { config });
  assert.equal(options.concurrentFragments, 32);
  assert.equal(options.experimentalFragments, true);
  assert.equal(parseCli([url, '--profile', 'fast', '--no-experimental-fragments'], { config }).experimentalFragments, false);
  assert.equal(parseCli([url, '-N', '64', '--experimental-fragments']).experimentalFragments, true);
  assert.throws(() => parseCli([url], { config: { concurrentFragments: 65, experimentalFragments: true } }), /between 1 and 64/);
});

test('normal, opted-in and read-only calls require no prompt', async () => {
  for (const options of [{ concurrentFragments: 16 }, { concurrentFragments: 64, experimentalFragments: true },
    { concurrentFragments: 32, dryRun: true }, { concurrentFragments: 32, listFormats: true },
    { concurrentFragments: 32, listSources: true }]) {
    await confirmExperimentalFragments([options], { confirm: () => assert.fail('unexpected prompt') });
  }
});

test('experimental terminal confirmation defaults to no and asks once for mixed retry items', async () => {
  const requests = [{ concurrentFragments: 16 }, { concurrentFragments: 17 }, { concurrentFragments: 64 }];
  let prompts = 0;
  await confirmExperimentalFragments(requests, { confirm: async prompt => {
    prompts++;
    assert.match(prompt, /64.*server limits.*\[y\/N\]/);
    return 'y';
  } });
  assert.equal(prompts, 1);
  for (const answer of ['', 'n', 'invalid']) {
    await assert.rejects(confirmExperimentalFragments(requests, { confirm: async () => answer }), /cancelled/);
  }
  assert.equal(requests[1].experimentalFragments, undefined);
});

test('noninteractive and JSON downloads require explicit acceptance; opt-in never bypasses cap', async () => {
  await assert.rejects(confirmExperimentalFragments([{ concurrentFragments: 32 }], {
    input: { isTTY: false }, output: { isTTY: false },
  }), /--experimental-fragments/);
  await assert.rejects(confirmExperimentalFragments([{ concurrentFragments: 32, json: true }], {
    input: { isTTY: true }, output: { isTTY: true },
  }), /--experimental-fragments/);
  await assert.rejects(confirmExperimentalFragments([{ concurrentFragments: 65, experimentalFragments: true }]), /between 1 and 64/);
});
