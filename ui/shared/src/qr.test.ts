import { describe, expect, it } from "vitest";

import { maximumQrVersion, qrCodeImage, qrCodeOf, qrCodePath, quietZoneModules } from "./qr";

const address = "https://example.org/btt/#";

describe("qrCodeOf", () => {
  it("draws a short trip as a small code", () => {
    const code = qrCodeOf(address, "Q".repeat(300));
    expect(code).not.toBeNull();
    expect(code!.version).toBeLessThan(maximumQrVersion);
    expect(code!.size).toBe(17 + 4 * code!.version);
  });

  it("refuses a trip that needs a bigger code than the app draws", () => {
    expect(qrCodeOf(address, "Q".repeat(5000))).toBeNull();
    // A character that the alphanumeric mode lacks cannot be drawn either.
    expect(qrCodeOf(address, "a b")).toBeNull();
  });

  it("has dark finder patterns in three corners and a light fourth corner", () => {
    const code = qrCodeOf(address, "Q".repeat(50))!;
    const last = code.size - 1;
    expect(code.isDark(0, 0)).toBe(true);
    expect(code.isDark(0, last)).toBe(true);
    expect(code.isDark(last, 0)).toBe(true);
    expect(code.isDark(last, last)).toBe(false);
    expect(code.isDark(-1, 0)).toBe(false);
    expect(code.isDark(0, code.size)).toBe(false);
  });
});

describe("the pictures of a code", () => {
  it("draws the path of the dark modules inside the quiet zone", () => {
    const code = qrCodeOf(address, "Q".repeat(50))!;
    const path = qrCodePath(code);
    expect(path.startsWith(`M${quietZoneModules} ${quietZoneModules}h7`)).toBe(true);
  });

  it("draws every dark module in the path and no other", () => {
    const code = qrCodeOf(address, "Q".repeat(200))!;
    const drawn = new Set<string>();
    // The path is a list of rectangles of one module high: M x y h n v1 h-n z.
    for (const [, x, y, length] of qrCodePath(code).matchAll(/M(\d+) (\d+)h(\d+)v1h-\d+z/g)) {
      for (let offset = 0; offset < Number(length); offset += 1) {
        drawn.add(`${Number(y) - quietZoneModules},${Number(x) - quietZoneModules + offset}`);
      }
    }
    const dark = new Set<string>();
    for (let row = 0; row < code.size; row += 1) {
      for (let column = 0; column < code.size; column += 1) {
        if (code.isDark(row, column)) {
          dark.add(`${row},${column}`);
        }
      }
    }
    expect(drawn).toEqual(dark);
  });

  it("draws black modules on white with a quiet zone around them", () => {
    const code = qrCodeOf(address, "Q".repeat(50))!;
    const image = qrCodeImage(code, 3);
    expect(image.width).toBe((code.size + 2 * quietZoneModules) * 3);
    // The corner of the quiet zone is white, and the first module of the code is black.
    expect(Array.from(image.data.slice(0, 4))).toEqual([255, 255, 255, 255]);
    const first = (quietZoneModules * 3 * image.width + quietZoneModules * 3) * 4;
    expect(Array.from(image.data.slice(first, first + 4))).toEqual([0, 0, 0, 255]);
  });
});
