// Small checks for data of unknown shape, such as the JSON of a trip file.
//
// A parser takes a value and returns it as the type it checked, or throws a `SnapshotLinkError`
// that names the place of the problem. Objects come out with only the fields the parser lists,
// so extra fields of a hostile file never reach the views.

import { malformed } from "./errors";

/** Checks a value and returns it as `T`. The path says where the value sits, for the message. */
export type Parser<T> = (value: unknown, path: string) => T;

/** Longest list a trip file may hold. */
const maximumListLength = 10_000;

/** Longest text a trip file may hold. */
const maximumTextLength = 1_000;

function describe(path: string, expected: string): Error {
  return malformed(`${path} is not ${expected}`);
}

export const boolean: Parser<boolean> = (value, path) => {
  if (typeof value !== "boolean") {
    throw describe(path, "true or false");
  }
  return value;
};

/** A whole number from `minimum` to `maximum`. */
export function integer(minimum: number, maximum: number): Parser<number> {
  return (value, path) => {
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum || value > maximum) {
      throw describe(path, `a whole number from ${minimum} to ${maximum}`);
    }
    return value;
  };
}

/** Any number from `minimum` to `maximum`. */
export function decimal(minimum: number, maximum: number): Parser<number> {
  return (value, path) => {
    if (typeof value !== "number" || !Number.isFinite(value) || value < minimum || value > maximum) {
      throw describe(path, `a number from ${minimum} to ${maximum}`);
    }
    return value;
  };
}

export const text: Parser<string> = (value, path) => {
  if (typeof value !== "string" || value.length > maximumTextLength) {
    throw describe(path, "a text");
  }
  return value;
};

/** A text that is a date such as `2026-10-14`. */
export const date: Parser<string> = (value, path) => {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(value) || Number.isNaN(Date.parse(value))) {
    throw describe(path, "a date");
  }
  return value;
};

/** One of a fixed set of texts. */
export function oneOf<T extends string>(...allowed: readonly T[]): Parser<T> {
  return (value, path) => {
    if (typeof value !== "string" || !allowed.includes(value as T)) {
      throw describe(path, `one of ${allowed.join(", ")}`);
    }
    return value as T;
  };
}

/** A value that may be `null`. A missing value counts as `null`. */
export function nullable<T>(parser: Parser<T>): Parser<T | null> {
  return (value, path) => (value === null || value === undefined ? null : parser(value, path));
}

/** A list of parsed items. */
export function list<T>(parser: Parser<T>): Parser<T[]> {
  return (value, path) => {
    if (!Array.isArray(value) || value.length > maximumListLength) {
      throw describe(path, "a list");
    }
    return value.map((item, index) => parser(item, `${path}[${index}]`));
  };
}

/** An object with the listed fields. Fields the shape lists but the object lacks are `undefined`. */
export function object<T>(shape: { [K in keyof T]-?: Parser<T[K]> }): Parser<T> {
  return (value, path) => {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw describe(path, "an object");
    }
    const record = value as Record<string, unknown>;
    const result: Record<string, unknown> = {};
    for (const key of Object.keys(shape)) {
      const parser = (shape as Record<string, Parser<unknown>>)[key]!;
      result[key] = parser(Object.hasOwn(record, key) ? record[key] : undefined, `${path}.${key}`);
    }
    return result as T;
  };
}

/**
 * A choice between variants, which the JSON writes as an object with one key, such as
 * `{"Nights": 2}`, or as a text for a variant without data, such as `"Daytime"`.
 */
export function variants<T>(
  unit: readonly string[],
  withData: { [name: string]: Parser<unknown> },
): Parser<T> {
  return (value, path) => {
    if (typeof value === "string" && unit.includes(value)) {
      return value as T;
    }
    if (typeof value === "object" && value !== null && !Array.isArray(value)) {
      const keys = Object.keys(value);
      const name = keys[0];
      const parser = name === undefined ? undefined : withData[name];
      if (keys.length === 1 && name !== undefined && parser !== undefined && Object.hasOwn(withData, name)) {
        return { [name]: parser((value as Record<string, unknown>)[name], `${path}.${name}`) } as T;
      }
    }
    throw describe(path, "a known choice");
  };
}
