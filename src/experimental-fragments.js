import { createInterface } from 'node:readline/promises';

export function validateFragments(value) {
  if (!Number.isInteger(value) || value < 1 || value > 64) {
    throw new Error('--concurrent-fragments must be a whole number between 1 and 64.');
  }
}

export async function confirmExperimentalFragments(requests, {
  input = process.stdin, output = process.stderr, confirm, signal,
} = {}) {
  for (const request of requests) validateFragments(request.concurrentFragments ?? 8);
  const pending = requests.filter(request => (request.concurrentFragments ?? 8) > 16
    && !request.experimentalFragments && !request.dryRun && !request.listQualities && !request.listFormats && !request.listSources);
  if (!pending.length) return;
  if (!confirm && (!input.isTTY || !output.isTTY || pending.some(request => request.json))) {
    throw new Error('17-64 concurrent fragments are experimental. Explicitly accept with --experimental-fragments or experimentalFragments: true in your config/profile.');
  }
  const maximum = Math.max(...pending.map(request => request.concurrentFragments));
  const prompt = `Experimental: up to ${maximum} concurrent fragments. More connections may trigger server limits and may not improve speed. Continue? [y/N] `;
  let answer;
  if (confirm) answer = await confirm(prompt);
  else {
    const rl = createInterface({ input, output });
    try { answer = await rl.question(prompt, { signal }); }
    finally { rl.close(); }
  }
  if (!/^y(?:es)?$/i.test(answer.trim())) throw new Error('Experimental fragment download cancelled.');
}
