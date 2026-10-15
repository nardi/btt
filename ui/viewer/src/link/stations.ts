// The dictionary of stations that links refer to by number.
//
// The dictionary is the file `crates/btt-model/data/stations.tsv`, which the encoder in Rust
// reads as well. It holds one station per line as its name, latitude and longitude in
// microdegrees and time zone, separated by tabs. A link names a station by its line, so lines are
// only ever appended.

/** A line of the dictionary. */
export interface DictionaryStation {
  readonly name: string;
  /** Latitude in microdegrees. */
  readonly latitude: number;
  /** Longitude in microdegrees. */
  readonly longitude: number;
  /** IANA time zone of the station. */
  readonly timeZone: string;
}

/** Reads the lines of a dictionary file. A line that is not a station is skipped. */
export function parseStationDictionary(text: string): DictionaryStation[] {
  const stations: DictionaryStation[] = [];
  for (const line of text.split("\n")) {
    const fields = line.split("\t");
    if (fields.length !== 4) {
      continue;
    }
    const [name, latitude, longitude, timeZone] = fields as [string, string, string, string];
    stations.push({ name, latitude: Number(latitude), longitude: Number(longitude), timeZone });
  }
  return stations;
}

let loaded: Promise<readonly DictionaryStation[]> | null = null;

/**
 * Returns the dictionary, which is fetched on first use as a file of its own, because most
 * pages of the viewer need it and the code of the viewer does not.
 */
export function loadStationDictionary(): Promise<readonly DictionaryStation[]> {
  loaded ??= import("../../../../crates/btt-model/data/stations.tsv?raw").then((module) =>
    parseStationDictionary(module.default),
  );
  return loaded;
}
