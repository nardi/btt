// The dictionary of trains that links refer to by number.
//
// The dictionary is the file `crates/btt-model/data/trains.tsv`, which the encoder in Rust reads
// as well. It holds one train per line as its operator, category and number, separated by tabs.
// The number may be empty. A link names a train by its line, so lines are only ever appended.

/** A line of the dictionary. */
export interface DictionaryTrain {
  /** The operator as the app names it, such as `db-fernverkehr`. */
  readonly operator: string;
  /** Product or category, such as "ICE". */
  readonly category: string;
  /** Train number within the category. */
  readonly number: string;
}

/** Reads the lines of a dictionary file. A line that is not a train is skipped. */
export function parseTrainDictionary(text: string): DictionaryTrain[] {
  const trains: DictionaryTrain[] = [];
  for (const line of text.split("\n")) {
    // The file may end lines with a carriage return when git checks it out on Windows.
    const fields = line.replace(/\r$/, "").split("\t");
    if (fields.length !== 3) {
      continue;
    }
    const [operator, category, number] = fields as [string, string, string];
    trains.push({ operator, category, number });
  }
  return trains;
}

let loaded: Promise<readonly DictionaryTrain[]> | null = null;

/**
 * Returns the dictionary, which is fetched on first use as a file of its own, because the code
 * of the viewer does not need it before a link opens.
 */
export function loadTrainDictionary(): Promise<readonly DictionaryTrain[]> {
  loaded ??= import("../../../../crates/btt-model/data/trains.tsv?raw").then((module) =>
    parseTrainDictionary(module.default),
  );
  return loaded;
}
