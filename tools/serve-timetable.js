// Serves a locally built timetable bundle the way the published manifest does, so the app can
// download it without GitHub.
//
// Build the files with `btt-builder --download-base-url <any address>`. That writes a
// `.bundle.zst` file and a `timetable-<version>.json` entry into the output directory. This
// script reads the entry, points its address at itself and serves the manifest at
// `/timetable.json`, with the compressed bundle next to it.
//
// To test the dialog on a first start, point the app at the server and at an empty data folder:
//
//   node tools/serve-timetable.js build-cache/output
//   BTT_TIMETABLE_MANIFEST_URL=http://127.0.0.1:8787/timetable.json \
//   BTT_DATA_DIRECTORY="$PWD/build-cache/app-data" npm run dev
//
// Use `--kbps 2000` to slow the download so the progress bar stays visible, `--fail-download` to
// answer the bundle with an error, and no server at all to test an unreachable manifest.

import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

const USAGE =
  'usage: node tools/serve-timetable.js <builder output directory> [--port N] [--kbps N] [--fail-download]';

/** Reads the command line into the directory and the options. */
function parseArguments(argv) {
  const options = { directory: null, port: 8787, kbps: 0, failDownload: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--port') {
      options.port = Number(argv[(index += 1)]);
    } else if (argument === '--kbps') {
      options.kbps = Number(argv[(index += 1)]);
    } else if (argument === '--fail-download') {
      options.failDownload = true;
    } else if (options.directory === null) {
      options.directory = argument;
    } else {
      console.error(USAGE);
      process.exit(1);
    }
  }
  if (options.directory === null || Number.isNaN(options.port) || Number.isNaN(options.kbps)) {
    console.error(USAGE);
    process.exit(1);
  }
  return options;
}

/** Returns the manifest entry with the highest version in the directory. */
function newestEntry(directory) {
  const names = fs
    .readdirSync(directory)
    .filter((name) => /^timetable-\d+\.json$/.test(name))
    .sort();
  if (names.length === 0) {
    console.error(`No timetable-<version>.json in ${directory}. Run btt-builder with --download-base-url.`);
    process.exit(1);
  }
  return JSON.parse(fs.readFileSync(path.join(directory, names[names.length - 1]), 'utf8'));
}

/** Sends a file in chunks of one tenth of a second at the given speed, or at once for zero. */
function sendFile(response, file, kbps) {
  response.writeHead(200, {
    'content-type': 'application/octet-stream',
    'content-length': fs.statSync(file).size,
  });
  if (kbps === 0) {
    fs.createReadStream(file).pipe(response);
    return;
  }
  const chunkBytes = Math.max(1, Math.floor((kbps * 1000) / 10));
  const stream = fs.createReadStream(file, { highWaterMark: chunkBytes });
  stream.on('data', (chunk) => {
    stream.pause();
    response.write(chunk, () => setTimeout(() => stream.resume(), 100));
  });
  stream.on('end', () => response.end());
  response.on('close', () => stream.destroy());
}

const options = parseArguments(process.argv.slice(2));
const entry = newestEntry(options.directory);
const fileName = path.basename(new URL(entry.url).pathname);
const bundleFile = path.join(options.directory, fileName);
if (!fs.existsSync(bundleFile)) {
  console.error(`The entry names ${fileName}, which is not in ${options.directory}.`);
  process.exit(1);
}
const origin = `http://127.0.0.1:${options.port}`;
const manifest = { bundles: [{ ...entry, url: `${origin}/${fileName}` }] };

http
  .createServer((request, response) => {
    console.log(`${request.method} ${request.url}`);
    if (request.url === '/timetable.json') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(manifest));
    } else if (request.url === `/${fileName}` && options.failDownload) {
      response.writeHead(500);
      response.end('failing on purpose');
    } else if (request.url === `/${fileName}`) {
      sendFile(response, bundleFile, options.kbps);
    } else {
      response.writeHead(404);
      response.end();
    }
  })
  .listen(options.port, '127.0.0.1', () => {
    console.log(`Serving version ${entry.version} (${entry.size_bytes} bytes) at ${origin}/timetable.json`);
  });
