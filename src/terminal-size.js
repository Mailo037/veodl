// Windows stderr can retain an old size between writes. Refresh the native TTY
// before reading it; custom/test streams retain their own live columns getter.
export function terminalColumns(stream) {
  if (process.platform === 'win32' && (stream === process.stdout || stream === process.stderr)) {
    try { stream._refreshSize?.(); } catch { /* Cosmetic fallback only. */ }
  }
  return Number.isInteger(stream.columns) && stream.columns > 0 ? stream.columns : 80;
}
