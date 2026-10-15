// The dictionary of city names that links refer to by number.
//
// The dictionary is the file `crates/btt-model/data/cities.tsv`, which the encoder in Rust reads
// as well. It holds one name per line as the name and the latitude and longitude of the city in
// microdegrees, separated by tabs. A link names a city by its line, so lines are only ever
// appended. The data comes from GeoNames, which is licensed under CC BY 4.0.

/** A line of the dictionary. */
export interface DictionaryCity {
  readonly name: string;
  /** Latitude in microdegrees. */
  readonly latitude: number;
  /** Longitude in microdegrees. */
  readonly longitude: number;
}

/** Size of the steps that a link places a place in, in microdegrees: about 11 meters. */
export const placeStepMicrodegrees = 100;

/** Returns a value in microdegrees in steps, rounded to the nearest step with a half rounded up. */
export function stepsOf(microdegrees: number): number {
  return Math.floor((microdegrees + placeStepMicrodegrees / 2) / placeStepMicrodegrees);
}

/** Reads the lines of a dictionary file. A line that is not a city is skipped. */
export function parseCityDictionary(text: string): DictionaryCity[] {
  const cities: DictionaryCity[] = [];
  for (const line of text.split("\n")) {
    const fields = line.split("\t");
    if (fields.length !== 3) {
      continue;
    }
    const [name, latitude, longitude] = fields as [string, string, string];
    cities.push({ name, latitude: Number(latitude), longitude: Number(longitude) });
  }
  return cities;
}

let loaded: Promise<readonly DictionaryCity[]> | null = null;

/** Returns the dictionary, which is fetched on first use as a file of its own. */
export function loadCityDictionary(): Promise<readonly DictionaryCity[]> {
  loaded ??= import("../../../../crates/btt-model/data/cities.tsv?raw").then((module) =>
    parseCityDictionary(module.default),
  );
  return loaded;
}
