// QR codes that carry a trip: the web address of the viewer in bytes and the trip after it in the
// alphanumeric mode of the code, which spends 5.5 bits on each character.

import { create, type QRCodeSegment } from "qrcode";

/**
 * Largest version of a QR code that the app draws, which is 117 by 117 modules. A bigger code
 * is hard to scan from a screen, so a trip that needs one is shared as a link or a file.
 */
export const maximumQrVersion = 25;

/** Modules of white around a code, which scanners need to find its edges. */
export const quietZoneModules = 4;

/** A QR code as a grid of modules. */
export interface QrCodeMatrix {
  /** Version of the code, from 1 to 40, which sets its size. */
  readonly version: number;
  /** Number of modules along a side, without the quiet zone. */
  readonly size: number;
  /** Returns whether a module is dark. Modules outside the grid are light. */
  isDark(row: number, column: number): boolean;
}

/**
 * Returns the QR code of a trip, or `null` when the trip needs a bigger code than
 * `maximumQrVersion`. The address goes in a segment of bytes and the payload in a segment of
 * alphanumeric characters, which only has to hold the characters of the QR alphabet.
 */
export function qrCodeOf(address: string, payload: string): QrCodeMatrix | null {
  let code;
  try {
    // The library takes the text of a segment as it is, though its types only name bytes.
    const segments = [
      { data: address, mode: "byte" },
      { data: payload, mode: "alphanumeric" },
    ] as unknown as QRCodeSegment[];
    code = create(segments, { errorCorrectionLevel: "M" });
  } catch {
    return null;
  }
  if (code.version > maximumQrVersion) {
    return null;
  }
  const modules = code.modules;
  return {
    version: code.version,
    size: modules.size,
    isDark: (row, column) =>
      row >= 0 && column >= 0 && row < modules.size && column < modules.size && modules.get(row, column) === 1,
  };
}

/**
 * Returns the dark modules of a code as the path of an SVG, with the origin at the corner of the
 * quiet zone. Runs of dark modules in a row make one rectangle.
 */
export function qrCodePath(matrix: QrCodeMatrix): string {
  const parts: string[] = [];
  for (let row = 0; row < matrix.size; row += 1) {
    let column = 0;
    while (column < matrix.size) {
      if (!matrix.isDark(row, column)) {
        column += 1;
        continue;
      }
      const start = column;
      while (column < matrix.size && matrix.isDark(row, column)) {
        column += 1;
      }
      const length = column - start;
      parts.push(`M${start + quietZoneModules} ${row + quietZoneModules}h${length}v1h-${length}z`);
    }
  }
  return parts.join("");
}

/** Pixels of a code: black modules on white, with the quiet zone around them. */
export interface QrCodeImage {
  readonly width: number;
  readonly height: number;
  /** RGBA values of the pixels, row after row. */
  readonly data: Uint8ClampedArray<ArrayBuffer>;
}

/** Draws a code as pixels, `scale` pixels to a module. */
export function qrCodeImage(matrix: QrCodeMatrix, scale: number): QrCodeImage {
  const side = (matrix.size + 2 * quietZoneModules) * scale;
  const data = new Uint8ClampedArray(new ArrayBuffer(side * side * 4)).fill(255);
  for (let y = 0; y < side; y += 1) {
    const row = Math.floor(y / scale) - quietZoneModules;
    for (let x = 0; x < side; x += 1) {
      const column = Math.floor(x / scale) - quietZoneModules;
      if (matrix.isDark(row, column)) {
        const offset = (y * side + x) * 4;
        data[offset] = 0;
        data[offset + 1] = 0;
        data[offset + 2] = 0;
      }
    }
  }
  return { width: side, height: side, data };
}
