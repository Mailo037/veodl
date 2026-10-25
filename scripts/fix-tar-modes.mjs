// npm pack on Windows cannot preserve Unix executable bits. Set them in tar.
import { readFileSync, writeFileSync } from 'node:fs';
import { gzipSync, gunzipSync } from 'node:zlib';
const file = process.argv[2];
const tar = gunzipSync(readFileSync(file));
for (let offset = 0; offset + 512 <= tar.length;) {
  const header = tar.subarray(offset, offset + 512);
  const name = header.subarray(0, 100).toString().replace(/\0.*$/s, '');
  if (!name) break;
  const size = parseInt(header.subarray(124, 136).toString().replace(/\0/g, '').trim(), 8) || 0;
  if (name.startsWith('package/native/') || name === 'package/bin/veo.js') {
    header.write('0000755\0', 100, 8, 'ascii');
    header.fill(32, 148, 156);
    const sum = header.reduce((total, byte) => total + byte, 0);
    header.write(sum.toString(8).padStart(6, '0') + '\0 ', 148, 8, 'ascii');
  }
  offset += 512 + Math.ceil(size / 512) * 512;
}
writeFileSync(file, gzipSync(tar));
