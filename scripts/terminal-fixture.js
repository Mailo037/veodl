// Offline fixture for the ConPTY resize smoke test; never downloads media.
import { createReporter } from '../src/progress.js';
const reporter = createReporter(process.stderr, { setTitle() {} });
reporter.configure({ color: false });
process.stdout.write('Shell before veo\r\n');
reporter.enableInline();
reporter.item(1, 1, 'https://example.test/' + 'signed-url'.repeat(150));
reporter.status('Reading video: done');
reporter.progress({ stream: 'Media', downloaded_bytes: 90 * 1024 ** 2,
  total_bytes_estimate: 1024 ** 3, speed: 12.6 * 1024 ** 2, eta: 77 });
process.stdin.setRawMode(true);
process.stdin.resume();
const stop = () => {
  reporter.output(process.stdout).write('Saved: fixture.mp4\n');
  reporter.complete(); reporter.dispose();
  process.stdin.setRawMode(false); process.stdin.pause();
  process.stdout.write('TERMINAL_RESTORED\r\n');
};
process.stdin.on('data', data => {
  if (data.includes(110)) reporter.name('Resize title ' + 'abcdefghij'.repeat(40));
  if (data.includes(113) || data.includes(3)) stop();
});
