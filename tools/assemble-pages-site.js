// Assembles the site that GitHub Pages serves. The viewer sits at the root, the web app in the
// `app` folder, and the newest timetable bundle in `app/timetable`, where the web app downloads
// it from. Pages sends no CORS headers for release assets, so the web app cannot read the
// bundle from the release and the site carries a copy of it.
//
// Build the viewer and the web app first. Then run
//
//   node tools/assemble-pages-site.js site
//
// Use `--viewer <directory>` and `--app <directory>` for builds in other places, and
// `--manifest <address>` for another manifest than the published one.

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const USAGE =
  'usage: node tools/assemble-pages-site.js <output directory> [--viewer N] [--app N] [--manifest URL]';

/** Where the timetable workflow publishes the manifest of the bundles. */
const PUBLISHED_MANIFEST_URL =
  'https://github.com/nardi/btt/releases/download/timetable-index/timetable.json';

/** Reads the command line into the output directory and the options. */
function parseArguments(argv) {
  const options = {
    output: null,
    viewer: 'ui/viewer/dist',
    app: 'ui/web/dist',
    manifest: PUBLISHED_MANIFEST_URL,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--viewer' || argument === '--app' || argument === '--manifest') {
      const value = argv[(index += 1)];
      if (value === undefined) {
        throw new Error(USAGE);
      }
      options[argument.slice(2)] = value;
    } else if (argument.startsWith('--') || options.output !== null) {
      throw new Error(USAGE);
    } else {
      options.output = argument;
    }
  }
  if (options.output === null) {
    throw new Error(USAGE);
  }
  return options;
}

/** Returns the newest bundle in the manifest that still covers some day from today on. */
function newestBundle(manifest) {
  const today = new Date().toISOString().slice(0, 10);
  const valid = manifest.bundles.filter((entry) => entry.valid_until >= today);
  valid.sort((left, right) => right.version - left.version);
  if (valid.length === 0) {
    throw new Error('the manifest lists no bundle that is still valid');
  }
  return valid[0];
}

/** Downloads a file and returns its bytes. */
async function download(address) {
  const response = await fetch(address);
  if (!response.ok) {
    throw new Error(`${address} answered ${response.status}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

const options = parseArguments(process.argv.slice(2));

fs.rmSync(options.output, { recursive: true, force: true });
fs.cpSync(options.viewer, options.output, { recursive: true });
fs.cpSync(options.app, path.join(options.output, 'app'), { recursive: true });

const manifest = JSON.parse((await download(options.manifest)).toString('utf8'));
const entry = newestBundle(manifest);
const bundle = await download(entry.url);
const digest = createHash('sha256').update(bundle).digest('hex');
if (bundle.length !== entry.size_bytes || digest !== entry.sha256) {
  throw new Error(`the bundle ${entry.version} does not match the manifest`);
}

// The entry points at a file beside the manifest, so the site never depends on the release.
const fileName = `timetable-${entry.version}.bundle.zst`;
const timetableDirectory = path.join(options.output, 'app', 'timetable');
fs.mkdirSync(timetableDirectory, { recursive: true });
fs.writeFileSync(path.join(timetableDirectory, fileName), bundle);
fs.writeFileSync(
  path.join(timetableDirectory, 'timetable.json'),
  JSON.stringify({ ...manifest, bundles: [{ ...entry, url: fileName }] }, null, 2),
);
console.log(`site written to ${options.output} with timetable ${entry.version}`);
