import { createHash } from 'node:crypto';

export const TEMPLATE_MARKER = '// veo: commented configuration template';

export const SOURCE_OPTIONS_GUIDE = String.raw`// veo: source discovery options v1
// Copy these settings into the global object or a named profile if desired.
// "deepScan": true,          // Verify every observed media candidate instead of the first 30
// "timeout": "2m",          // Discovery deadline: 5s to 10m; plain numbers mean seconds
// "listSources": false,     // true = list sources and exit for single-URL calls
// "autoListSources": true,  // Search after extraction fails; false disables fallback
// Command-line flags override these values, including --no-deep-scan and --no-list-sources.
`;

const PROFILE_OPTIONS_MARKER = '// veo: profile download options v5';
export const PROFILE_OPTIONS_GUIDE = String.raw`${PROFILE_OPTIONS_MARKER}
// Any global option can also be placed inside a profile.
// Copy the settings you want into "profiles" > "default" or a named profile.
// Remove the leading // on copied settings; separate entries with commas.
// Speed without reducing the selected video quality:
// "quality": "best",
// "concurrentDownloads": 2,    // Parallel URLs/batch items: 1-4; default 2
// "adaptiveConcurrency": true, // Reduce parallelism and retry transient errors
// "concurrentFragments": 8,    // DASH/HLS fragments: 1-64; default 8
// Values 17-64 are experimental and require terminal confirmation before download.
// "experimentalFragments": false, // true explicitly accepts 17-64, including in scripts
// More connections may trigger server limits and do not guarantee better speed.
// "deepScan": true,           // Check every observed source candidate
// "timeout": "2m",           // Source-search deadline, 5s to 10m
// "listSources": false,      // List sources and exit for one URL
// "autoListSources": true,   // Search after extraction fails (default)
// "playlist": true,            // Enable only for playlist downloads
// "playlistConcurrency": 2,    // Simultaneous entries: 1-4; default 2
// "checkSpace": true,          // Estimate free cache/output space first
// "timings": true,             // Show time spent in each phase
// "color": true,               // Gray details and colored results; false = plain text
// Applies to veo's own terminal output, including stats/history/doctor.
// Each profile can set color independently; NO_COLOR always disables styling.
// Example inside profiles: "plain": { "color": false }
// Use: veo stats --profile plain or veo URL --profile plain
// "folderTemplate": "{channel}/{year}",
// "filenameTemplate": "{index} - {title}", // Without extension; do not combine with rename
// "resume": true,
// "incognito": false,        // Save only media; no run ID, history, stats or retry data
// "neutralFilename": false,  // Optional random filenames; ignores rename and naming templates
// "skipExisting": true
// "verify": false,           // Probe saved media with FFprobe; no full decode
// Lossless container change (the selected codecs must fit the container):
// "format": "mkv",
// "compatible": false,        // Opt-in H.264/AAC MP4; converts only when needed
// "recode": false
// Compatibility profile to add inside profiles:
// "kompatibel": { "audio": false, "format": "mp4", "compatible": true, "recode": false }
// Use: veo URL --profile kompatibel (or simply veo URL --compatible).
// Optional conversion instead: "format": "webm", "recode": true
// Conversion can be slower and lose quality; recode is for video only.
// Sequential downloads: "concurrentDownloads": 1, "concurrentFragments": 1, "playlistConcurrency": 1
// Example named profile inside the profiles object:
// "fast": { "quality": "best", "concurrentFragments": 8, "playlistConcurrency": 2, "resume": true }
// Use with: veo URL --profile fast (add --playlist for a playlist URL).
// Inspect: veo config show --profile fast | Validate: veo config check
// Template fields: {title}, {id}, {channel}, {year}, {playlist}, {index}.
// Missing values use Unknown channel/year, No playlist, unknown ID, index 001.
`;

const TEMPLATE_SOURCE = String.raw`${TEMPLATE_MARKER}
// Save and close the editor. Settings apply the next time you run veo.
// Comments using // or /* ... */ are supported. Command-line options take priority.
// To enable an option, remove its leading // and adjust the value.
// Separate active entries with commas. Do not add a comma after the last entry.
{
  // Output directory: use / or double backslashes, for example "D:\\Videos".
  // Without output, downloads are saved in the current working directory.
  // "output": "D:/Videos",
  // Filename without extension. Every * inserts the original video title.
  // "rename": "movie_*",

  // Maximum video resolution: best, 2160p, 1440p, 1080p, 720p, ...
  // "quality": "1080p",
  // "closestQuality": false, // Allow the nearest available resolution
  // Video format: mp4, mkv, webm, mov. Lossless remux; codecs must fit the container.
  // "format": "mp4",
  // "recode": false,           // Explicit video conversion; may lose quality
  // "playlistConcurrency": 2, // Concurrent playlist entries: 1 to 4
  // "playlistItems": "1-5",  // Download only these playlist entries
  // "section": "*10:00-12:00", // Download only a time range
  // "open": false,              // Open the completed file automatically
  // "resume": true,             // Resume interrupted downloads
  // "incognito": true,          // Save only media; no run ID, history, stats or retry data
  // "neutralFilename": false,  // Set true for random filenames without title or template folders
  // "skipExisting": true,       // Skip previously saved downloads
  // "verify": false,           // Check saved media container and tracks with FFprobe
  // "concurrentFragments": 8,   // Concurrent fragments: 1 to 64; default 8
  // Values above 16 require confirmation or the explicit opt-in below.
  // "experimentalFragments": false, // true accepts experimental values 17-64
  // "cookies": "D:/cookies.txt", // Netscape-format cookies file
  // "cookiesFromBrowser": "firefox", // Use your browser's cookies
  // "sponsorblockRemove": "sponsor", // Remove matching SponsorBlock segments
  // "json": false,            // Print machine-readable output for downloads

${SOURCE_OPTIONS_GUIDE}

  // Subtitles and additional information:
  // "subs": true,              // Download subtitles; defaults to English
  // "autoSubs": false,         // Include automatically generated subtitles
  // "subLangs": "de,en",        // Enable subtitles for these languages
  // "subFormat": "srt/vtt/best", // Prefer these available subtitle formats
  // "embedSubs": true,          // Embed subtitles in the video
  // "embedMetadata": true,      // Embed the title, date and other metadata
  // "embedThumbnail": true,     // Embed the thumbnail

  // The default profile is used automatically unless you select another profile.
  // Use veo profile NAME to make any named profile the default for future commands.
  // The active choice is stored at the top level: "activeProfile": "music",
${PROFILE_OPTIONS_GUIDE}
  // Select other profiles with --profile NAME or in the interactive wizard.
  // Example: veo "https://example.com/video.mp4" --profile music
  "profiles": {
    "default": {
      // "color": true, // Set false to disable terminal styling for this profile.
      // Add everyday defaults here, for example: "quality": "1080p"
      // Empty means use the global settings above and veo's built-in defaults.
    },
    "kompatibel": {
      "audio": false,
      "format": "mp4",
      "compatible": true,
      "recode": false
    },
    "music": {
      // Audio only. Formats: mp3, m4a, aac, opus, flac, wav.
      "audio": true,
      "format": "mp3"
    },
    "archive": {
      "quality": "1080p",
      "embedMetadata": true,
      "subLangs": "de,en"
    }
  }
}
`;

// The guide revision follows its content so the editor can offer an update.
const TEMPLATE_REVISION = createHash('sha256').update(TEMPLATE_SOURCE).digest('hex').slice(0, 12);
const REFERENCE_BEGIN = '// veo: template reference begin';
const REFERENCE_END = '// veo: template reference end';
export const CONFIG_TEMPLATE = `{
  "profiles": {
    "default": {},
    "kompatibel": { "audio": false, "format": "mp4", "compatible": true, "recode": false },
    "music": { "audio": true, "format": "mp3" },
    "archive": { "quality": "1080p", "embedMetadata": true, "subLangs": "de,en" }
  }
}
`;

// Kept in the program. It is inserted into a config only when requested.
export const CONFIG_GUIDE = `${REFERENCE_BEGIN} ${TEMPLATE_REVISION}\n// Current veo options. Copy only the settings you want into the JSON below.\n${TEMPLATE_SOURCE.split('\n').slice(1).map(line => line.startsWith('//') ? line : `// ${line}`).join('\n').trimEnd()}\n${REFERENCE_END}\n`;

function generatedRange(text) {
  const begin = text.indexOf(REFERENCE_BEGIN);
  const end = begin < 0 ? -1 : text.indexOf(REFERENCE_END, begin);
  if (begin < 0) {
    const legacyHeader = /^\uFEFF?\/\/ veo: commented configuration template\r?\n\/\/ Reference: copy any examples/;
    const settingsMarker = '// Your existing settings:';
    const markerAt = legacyHeader.test(text) ? text.indexOf(settingsMarker) : -1;
    if (markerAt < 0) return null;
    const lineEnd = text.indexOf('\n', markerAt);
    return { start: 0, stop: lineEnd < 0 ? text.length : lineEnd + 1, revision: 'legacy' };
  }
  if (end < 0) return null;
  let start = begin;
  const before = text.slice(0, begin);
  if (/^\uFEFF?\/\/ veo: commented configuration template\r?\n$/.test(before)) start = 0;
  const lineEnd = text.indexOf('\n', end);
  let stop = lineEnd < 0 ? text.length : lineEnd + 1;
  if (text.slice(stop).startsWith('// Your existing settings:\n')) stop += '// Your existing settings:\n'.length;
  return { start, stop, revision: text.slice(begin, text.indexOf('\n', begin) < 0 ? text.length : text.indexOf('\n', begin)) };
}

export function configGuideState(text) {
  const range = generatedRange(text);
  if (!range) return text.includes(REFERENCE_BEGIN) || text.includes('// Reference: copy any examples') ? 'incomplete' : 'absent';
  return range.revision === `${REFERENCE_BEGIN} ${TEMPLATE_REVISION}` ? 'current' : 'outdated';
}

export function addConfigGuide(text) {
  const range = generatedRange(text);
  if (!range && configGuideState(text) === 'incomplete') return text;
  const bom = text.startsWith('\uFEFF') ? '\uFEFF' : '';
  if (range) return (range.start === 0 ? bom : text.slice(0, range.start)) + CONFIG_GUIDE + text.slice(range.stop);
  return bom + CONFIG_GUIDE + text.slice(bom.length);
}

export function removeConfigGuide(text) {
  const range = generatedRange(text);
  return range ? (range.start === 0 && text.startsWith('\uFEFF') ? '\uFEFF' : text.slice(0, range.start)) + text.slice(range.stop) : text;
}

// Remove comments only outside JSON strings. Whitespace replacement preserves
// token boundaries, so malformed input cannot become valid by joining tokens.
export function stripConfigComments(text) {
  text = text.replace(/^\uFEFF/, '');
  let result = '', quoted = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (quoted) {
      result += char;
      if (char === '\\' && i + 1 < text.length) result += text[++i];
      else if (char === '"') quoted = false;
    } else if (char === '"') { quoted = true; result += char; }
    else if (char === '/' && text[i + 1] === '/') {
      result += '  '; i++;
      while (i + 1 < text.length && !'\r\n'.includes(text[i + 1])) { result += ' '; i++; }
    } else if (char === '/' && text[i + 1] === '*') {
      const commentStart = i;
      result += '  '; i++;
      let closed = false;
      while (++i < text.length) {
        if (text[i] === '*' && text[i + 1] === '/') { result += '  '; i++; closed = true; break; }
        result += '\r\n'.includes(text[i]) ? text[i] : ' ';
      }
      if (!closed) throw Object.assign(new Error('Unterminated block comment; add */ to close it.'), { position: commentStart });
    } else result += char;
  }
  return result;
}
