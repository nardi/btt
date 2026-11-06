// What the web app keeps in the browser between visits: the trips and the timetable.

const DATABASE_NAME = "btt";
const STORE_NAME = "kv";
const STATE_KEY = "state";
const BUNDLE_KEY = "bundle";
/** Prefix of the keys of planned legs, which hold the timetable namespace and the cache key. */
const LEG_PREFIX = "leg:";

/** The unpacked timetable bundle that the planner uses. */
export interface StoredBundle {
  /** Version of the data as `yyyymmddNN`. */
  version: number;
  /** The bundle file. */
  bytes: Uint8Array;
}

/** Opens the database, creating the store on the first visit. */
function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DATABASE_NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE_NAME);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/** Reads one value, or `undefined` when the store has none. */
async function read<T>(key: string): Promise<T | undefined> {
  const database = await openDatabase();
  try {
    return await new Promise<T | undefined>((resolve, reject) => {
      const request = database.transaction(STORE_NAME).objectStore(STORE_NAME).get(key);
      request.onsuccess = () => resolve(request.result as T | undefined);
      request.onerror = () => reject(request.error);
    });
  } finally {
    database.close();
  }
}

/** Writes one value. */
async function write(key: string, value: unknown): Promise<void> {
  const database = await openDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(STORE_NAME, "readwrite");
      transaction.objectStore(STORE_NAME).put(value, key);
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error);
    });
  } finally {
    database.close();
  }
}

/** Returns the text the backend saved last, if any. */
export function loadState(): Promise<string | undefined> {
  return read<string>(STATE_KEY);
}

/** Saves the text of the backend state. */
export function saveState(json: string): Promise<void> {
  return write(STATE_KEY, json);
}

/** Returns the installed timetable bundle, if any. */
export function loadBundle(): Promise<StoredBundle | undefined> {
  return read<StoredBundle>(BUNDLE_KEY);
}

/** Replaces the installed timetable bundle. */
export function saveBundle(bundle: StoredBundle): Promise<void> {
  return write(BUNDLE_KEY, bundle);
}

/** A planned leg read back from the database, with its cache key and planned result as JSON. */
export interface StoredLegEntry {
  /** The cache key of the leg, in decimal. */
  key: string;
  /** The planned leg as JSON. */
  result: string;
}

/** Saves a planned leg under its timetable namespace and cache key. */
export function saveLeg(namespace: string, key: string, json: string): Promise<void> {
  return write(`${LEG_PREFIX}${namespace}:${key}`, json);
}

/** Returns the planned legs of one timetable namespace. */
export async function loadLegs(namespace: string): Promise<StoredLegEntry[]> {
  const prefix = `${LEG_PREFIX}${namespace}:`;
  const entries = await entriesInRange(IDBKeyRange.bound(prefix, `${prefix}\uffff`));
  return entries.map((entry) => ({ key: entry.key.slice(prefix.length), result: entry.value }));
}

/** Deletes the planned legs of every timetable namespace except one, which can never match again. */
export async function deleteLegsOutside(namespace: string): Promise<void> {
  const keep = `${LEG_PREFIX}${namespace}:`;
  const database = await openDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(STORE_NAME, "readwrite");
      const request = transaction
        .objectStore(STORE_NAME)
        .openCursor(IDBKeyRange.bound(LEG_PREFIX, `${LEG_PREFIX}\uffff`));
      request.onsuccess = () => {
        const cursor = request.result;
        if (cursor === null) {
          return;
        }
        if (!String(cursor.key).startsWith(keep)) {
          cursor.delete();
        }
        cursor.continue();
      };
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error);
    });
  } finally {
    database.close();
  }
}

/** Returns the keys and values of the entries whose keys lie in a range. */
async function entriesInRange(range: IDBKeyRange): Promise<{ key: string; value: string }[]> {
  const database = await openDatabase();
  try {
    return await new Promise((resolve, reject) => {
      const entries: { key: string; value: string }[] = [];
      const request = database.transaction(STORE_NAME).objectStore(STORE_NAME).openCursor(range);
      request.onsuccess = () => {
        const cursor = request.result;
        if (cursor === null) {
          resolve(entries);
          return;
        }
        entries.push({ key: String(cursor.key), value: cursor.value as string });
        cursor.continue();
      };
      request.onerror = () => reject(request.error);
    });
  } finally {
    database.close();
  }
}
