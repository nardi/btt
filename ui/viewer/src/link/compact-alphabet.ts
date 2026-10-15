// The alphabet of the compact link: 44,190 letters of the Basic Multilingual Plane.
//
// The alphabet is the file `crates/btt-model/data/compact-alphabet.txt`, which lists its ranges
// in order as two hexadecimal code points per line. The encoder in Rust reads the same file.
// Every letter is one UTF-16 unit and carries 15.4 bits. Links depend on the exact alphabet, so
// the file never changes.

/** A range of the alphabet. */
interface LetterRange {
  readonly first: number;
  readonly last: number;
  /** Position in the alphabet of the first letter of the range. */
  readonly start: number;
}

/** The letters of the alphabet file in order. */
export class CompactAlphabet {
  readonly size: number;

  constructor(private readonly ranges: readonly LetterRange[]) {
    const last = ranges.at(-1);
    this.size = last === undefined ? 0 : last.start + (last.last - last.first + 1);
  }

  /** Returns the position of a letter, or `undefined` when the alphabet lacks it. */
  indexOf(letter: string): number | undefined {
    const code = letter.codePointAt(0);
    if (code === undefined || letter.length !== 1) {
      return undefined;
    }
    // The last range that starts at or before the code point.
    let low = 0;
    let high = this.ranges.length - 1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      const range = this.ranges[middle]!;
      if (code < range.first) {
        high = middle - 1;
      } else if (code > range.last) {
        low = middle + 1;
      } else {
        return range.start + (code - range.first);
      }
    }
    return undefined;
  }

  /** Returns the letter at a position, or `undefined` when the position is past the end. */
  letterAt(index: number): string | undefined {
    let low = 0;
    let high = this.ranges.length - 1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      const range = this.ranges[middle]!;
      if (index < range.start) {
        high = middle - 1;
      } else if (index > range.start + (range.last - range.first)) {
        low = middle + 1;
      } else {
        return String.fromCharCode(range.first + (index - range.start));
      }
    }
    return undefined;
  }
}

/** Reads the lines of the alphabet file. */
export function parseCompactAlphabet(text: string): CompactAlphabet {
  const ranges: LetterRange[] = [];
  let size = 0;
  for (const line of text.split("\n")) {
    const match = /^([0-9A-F]{4})-([0-9A-F]{4})$/.exec(line);
    if (match === null) {
      continue;
    }
    const first = Number.parseInt(match[1]!, 16);
    const last = Number.parseInt(match[2]!, 16);
    if (first > last) {
      continue;
    }
    ranges.push({ first, last, start: size });
    size += last - first + 1;
  }
  return new CompactAlphabet(ranges);
}

let loaded: Promise<CompactAlphabet> | null = null;

/** Returns the alphabet, which is fetched on first use as a file of its own. */
export function loadCompactAlphabet(): Promise<CompactAlphabet> {
  loaded ??= import("../../../../crates/btt-model/data/compact-alphabet.txt?raw").then((module) =>
    parseCompactAlphabet(module.default),
  );
  return loaded;
}
