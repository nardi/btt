// Runs the backend. The page sends commands here, and this worker passes them to the
// WebAssembly module. It also does what needs the network or the browser: it downloads the
// timetable, looks up places and keeps the trips in IndexedDB.

import { decompress } from "fzstd";

import init, * as backend from "./wasm/btt_web.js";
import {
  deleteLegsOutside,
  loadBundle,
  loadLegs,
  loadState,
  saveBundle,
  saveLeg,
  saveState,
} from "./storage";

/** A command from the page. */
interface CommandRequest {
  id: number;
  command: string;
  /** The arguments as JSON. */
  args: string;
}

/** Where the site keeps the manifest of the timetables, beside the app. */
const MANIFEST_URL = new URL(`${import.meta.env.BASE_URL}timetable/timetable.json`, self.location.origin);

/** Language for the names of places. */
const LANGUAGE = "en";

/** The Photon service allows one request per second. */
const PHOTON_INTERVAL_MILLISECONDS = 1100;

/** Prefix of the text that stands for a channel in the arguments of a command. */
const CHANNEL_PREFIX = "__CHANNEL__:";

/** The manifest as the server last sent it. */
let manifestText: string | undefined;

/** Writes the state once changes have stopped for a moment. */
let pendingStateSave: ReturnType<typeof setTimeout> | undefined;

/** A reply to the page. */
function reply(message: object): void {
  self.postMessage(message);
}

/** Sends a message to a channel of the page. */
function sendToChannel(channelId: number, json: string): void {
  reply({ kind: "channel", channelId, json });
}

/** Starts the module and loads what the last visit left. */
async function start(): Promise<void> {
  await init();
  const state = await loadState();
  backend.start(
    sendToChannel,
    (json: string) => {
      clearTimeout(pendingStateSave);
      pendingStateSave = setTimeout(() => void saveState(json), 200);
    },
    (name: string, contents: string) => reply({ kind: "file", name, contents }),
    (namespace: string, key: string, json: string) =>
      void saveLeg(namespace, key, json).catch((error) =>
        console.warn("could not save a planned leg", error),
      ),
    state,
  );
  const bundle = await loadBundle();
  if (bundle !== undefined) {
    try {
      await activateBundle(bundle.bytes);
    } catch (error) {
      console.warn("the stored timetable does not load", error);
    }
  }
}

/** Makes the planner use a timetable bundle, and loads the planned legs that belong to it. */
async function activateBundle(bytes: Uint8Array): Promise<void> {
  backend.install_bundle(bytes);
  const namespace = backend.leg_cache_namespace();
  await deleteLegsOutside(namespace);
  backend.load_leg_results(JSON.stringify(await loadLegs(namespace)));
}

const started = start();

/** An error that the page shows as the failure of a timetable download. */
function timetableFailure(message: string): string {
  return JSON.stringify({ Timetable: { message } });
}

/** Returns the timetable status as the backend sees it. */
async function timetableStatus(): Promise<string> {
  return backend.invoke("timetable_status", "{}");
}

/** Asks the server for the manifest of the timetables. */
async function checkTimetable(): Promise<string> {
  try {
    const response = await fetch(MANIFEST_URL, { cache: "no-cache" });
    if (!response.ok) {
      throw new Error(`the server answered ${response.status}`);
    }
    manifestText = await response.text();
    backend.record_manifest(manifestText, undefined);
  } catch (error) {
    backend.record_manifest(undefined, error instanceof Error ? error.message : String(error));
  }
  return timetableStatus();
}

/** One entry of the manifest. */
interface ManifestEntry {
  version: number;
  url: string;
  size_bytes: number;
  sha256: string;
}

/** Returns the SHA-256 digest of some bytes as lowercase hexadecimal. */
async function sha256Hex(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Downloads the bundle at an address, reporting how many bytes have arrived. */
async function downloadBytes(
  url: URL,
  expectedBytes: number,
  onProgress: (receivedBytes: number) => void,
): Promise<Uint8Array<ArrayBuffer>> {
  const response = await fetch(url);
  if (!response.ok || response.body === null) {
    throw new Error(`the server answered ${response.status}`);
  }
  const bytes = new Uint8Array(expectedBytes);
  const reader = response.body.getReader();
  let received = 0;
  let lastReport = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    if (received + value.length > expectedBytes) {
      throw new Error("the file is bigger than the manifest says");
    }
    bytes.set(value, received);
    received += value.length;
    const now = Date.now();
    if (now - lastReport > 200) {
      lastReport = now;
      onProgress(received);
    }
  }
  if (received !== expectedBytes) {
    throw new Error("the file is smaller than the manifest says");
  }
  onProgress(received);
  return bytes;
}

/** Downloads the newest timetable that the app can read and starts planning with it. */
async function downloadTimetable(argumentsJson: string): Promise<string> {
  const progressText = (JSON.parse(argumentsJson) as { progress: string }).progress;
  const channelId = Number(progressText.slice(CHANNEL_PREFIX.length));
  const report = (stage: "Downloading" | "Unpacking", received: number, total: number) =>
    sendToChannel(
      channelId,
      JSON.stringify({ stage, received_bytes: received, total_bytes: total }),
    );
  if (manifestText === undefined) {
    await checkTimetable();
  }
  const status = JSON.parse(await timetableStatus()) as {
    available: { version: number; readable: boolean } | null;
  };
  if (manifestText === undefined || status.available === null) {
    throw timetableFailure("the server lists no timetable");
  }
  if (!status.available.readable) {
    throw timetableFailure("this version of the app cannot read the newest timetable");
  }
  const wanted = status.available.version;
  const entry = (JSON.parse(manifestText) as { bundles: ManifestEntry[] }).bundles.find(
    (candidate) => candidate.version === wanted,
  );
  if (entry === undefined) {
    throw timetableFailure("the manifest lost the timetable");
  }
  try {
    const compressed = await downloadBytes(
      new URL(entry.url, MANIFEST_URL),
      entry.size_bytes,
      (received) => report("Downloading", received, entry.size_bytes),
    );
    report("Unpacking", entry.size_bytes, entry.size_bytes);
    if ((await sha256Hex(compressed)) !== entry.sha256) {
      throw new Error("the download does not match its checksum");
    }
    const bundle = decompress(compressed);
    await activateBundle(bundle);
    await saveBundle({ version: entry.version, bytes: bundle });
  } catch (error) {
    throw timetableFailure(error instanceof Error ? error.message : String(error));
  }
  return timetableStatus();
}

/** The time the last Photon request was sent. */
let lastPhotonRequest = 0;

/** Waits for the turn of the next Photon request. Requests are spaced out one after another. */
let photonQueue: Promise<unknown> = Promise.resolve();

/** Fetches a Photon answer as text, no sooner than a second after the request before it. */
function photonFetch(url: string): Promise<string> {
  const turn = photonQueue.then(async () => {
    const wait = lastPhotonRequest + PHOTON_INTERVAL_MILLISECONDS - Date.now();
    if (wait > 0) {
      await new Promise((resolve) => setTimeout(resolve, wait));
    }
    lastPhotonRequest = Date.now();
    const response = await fetch(url);
    if (!response.ok) {
      throw new Error(`the server answered ${response.status}`);
    }
    return response.text();
  });
  photonQueue = turn.catch(() => undefined);
  return turn;
}

/** Answers of earlier name searches. */
const searchCache = new Map<string, string>();

/** Searches places by name. */
async function searchPlaces(argumentsJson: string): Promise<string> {
  const query = (JSON.parse(argumentsJson) as { query: string }).query.trim().toLowerCase();
  if (query === "") {
    return "[]";
  }
  const cached = searchCache.get(query);
  if (cached !== undefined) {
    return cached;
  }
  const url = backend.photon_search_url(query, LANGUAGE);
  if (url === undefined) {
    return "[]";
  }
  try {
    const results = backend.photon_search_results(await photonFetch(url));
    searchCache.set(query, results);
    return results;
  } catch (error) {
    throw JSON.stringify({
      PlaceSearch: { message: error instanceof Error ? error.message : String(error) },
    });
  }
}

/** The trip in the answer to an edit, which holds the places that still need a name. */
interface EditAnswer {
  trip?: { id: string };
}

/**
 * Names the places of a trip that have the name made from their coordinates. Looks up the town
 * of each in the background, and the backend renames the place if the traveller left it alone.
 */
function nameProvisionalPlaces(answerJson: string): void {
  const answer = JSON.parse(answerJson) as EditAnswer | null;
  if (answer?.trip === undefined) {
    return;
  }
  const tripId = answer.trip.id;
  const places = JSON.parse(backend.provisional_places(JSON.stringify(answer.trip))) as {
    placeId: unknown;
    coordinates: unknown;
    latitude: number;
    longitude: number;
  }[];
  for (const place of places) {
    const url = backend.photon_reverse_url(place.latitude, place.longitude, LANGUAGE);
    if (url === undefined) {
      continue;
    }
    void photonFetch(url)
      .then((body) => backend.photon_reverse_name(body))
      .then((name) => {
        if (name === undefined) {
          return undefined;
        }
        return backend.invoke(
          "web_name_place",
          JSON.stringify({ tripId, placeId: place.placeId, coordinates: place.coordinates, name }),
        );
      })
      .catch((error) => console.warn("could not name a place", error));
  }
}

/** Runs a command. */
async function run(request: CommandRequest): Promise<string> {
  switch (request.command) {
    case "search_places":
      return searchPlaces(request.args);
    case "check_timetable":
      return checkTimetable();
    case "download_timetable":
      return downloadTimetable(request.args);
    case "apply_trip_edit":
    case "undo_trip_edit":
    case "redo_trip_edit":
    case "add_place_at_coordinates": {
      const answer = await backend.invoke(request.command, request.args);
      nameProvisionalPlaces(answer);
      return answer;
    }
    default:
      return backend.invoke(request.command, request.args);
  }
}

self.onmessage = async (event: MessageEvent<CommandRequest>) => {
  const request = event.data;
  try {
    await started;
    reply({ kind: "result", id: request.id, json: await run(request) });
  } catch (error) {
    // The backend rejects with text, and so do the commands above. Anything else is a bug.
    const message = typeof error === "string" ? error : String(error);
    reply({ kind: "failure", id: request.id, message });
  }
};
