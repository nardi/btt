// Reads a QR code from a picture or from a frame of the camera.

import jsQR from "jsqr";

/** Pixels of a picture, as `ImageData` holds them. */
export interface Pixels {
  readonly data: Uint8ClampedArray;
  readonly width: number;
  readonly height: number;
}

/**
 * The longest sides, in pixels, that a picture is tried at. A scanner finds the squares of a code
 * best when a module is a few pixels wide, so a large photo is shrunk, and a picture that holds a
 * code at a small size is tried as it is.
 */
const attemptedSides = [1600, 1000, 640, 400];

/** Returns the text of the QR code in the pixels, or `null` when there is none. */
export function readQrCode(pixels: Pixels): string | null {
  // A code that is light on dark, as a screenshot in a dark theme shows it, needs the inverse.
  const found = jsQR(pixels.data, pixels.width, pixels.height, { inversionAttempts: "attemptBoth" });
  return found === null ? null : found.data;
}

/** Returns the text of the QR code in a picture file, or `null` when there is none. */
export async function readQrCodeFromImage(file: Blob): Promise<string | null> {
  const bitmap = await createImageBitmap(file);
  try {
    const longest = Math.max(bitmap.width, bitmap.height);
    const sides = [...new Set(attemptedSides.map((side) => Math.min(side, longest)))];
    for (const side of sides) {
      const scale = side / longest;
      const width = Math.max(1, Math.round(bitmap.width * scale));
      const height = Math.max(1, Math.round(bitmap.height * scale));
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (context === null) {
        return null;
      }
      context.drawImage(bitmap, 0, 0, width, height);
      const text = readQrCode(context.getImageData(0, 0, width, height));
      if (text !== null) {
        return text;
      }
    }
    return null;
  } finally {
    bitmap.close();
  }
}

/**
 * Returns the payload of a trip that the text of a QR code holds, or `null` when the code is
 * something else. A trip code holds the address of the viewer with the payload after the `#`, or
 * only the payload.
 */
export function tripPayloadOfQr(text: string): string | null {
  const hash = text.indexOf("#");
  if (hash >= 0) {
    return text.length > hash + 1 ? text.slice(hash + 1) : null;
  }
  return /^[QR][0-9A-Z$*+\-./:]{3,}$/.test(text) ? text : null;
}
