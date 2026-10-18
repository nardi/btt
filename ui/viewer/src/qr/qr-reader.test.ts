import { qrCodeImage, qrCodeOf } from "@btt/shared/qr";
import { describe, expect, it } from "vitest";
import demo from "../fixtures/demo.json";
import { decodeSnapshotLink } from "../link/snapshot-link";
import { readQrCode, tripPayloadOfQr } from "./qr-reader";

const address = "https://nardi.github.io/btt/#";

describe("reading a QR code that holds a trip", () => {
  it("reads the code the app draws, and the trip in it", async () => {
    const matrix = qrCodeOf(address, demo.qr_payload);
    expect(matrix).not.toBeNull();
    const text = readQrCode(qrCodeImage(matrix!, 6));
    expect(text).toBe(address + demo.qr_payload);
    const payload = tripPayloadOfQr(text!);
    expect(payload).toBe(demo.qr_payload);
    expect(await decodeSnapshotLink(payload!)).toEqual(demo.document.snapshot);
  });

  it("reads a code drawn small and a code that is light on dark", () => {
    const matrix = qrCodeOf(address, demo.qr_payload)!;
    expect(readQrCode(qrCodeImage(matrix, 3))).toBe(address + demo.qr_payload);
    const image = qrCodeImage(matrix, 6);
    const inverted = new Uint8ClampedArray(image.data.length);
    for (let index = 0; index < image.data.length; index += 4) {
      inverted[index] = 255 - image.data[index]!;
      inverted[index + 1] = 255 - image.data[index + 1]!;
      inverted[index + 2] = 255 - image.data[index + 2]!;
      inverted[index + 3] = 255;
    }
    expect(readQrCode({ data: inverted, width: image.width, height: image.height })).toBe(
      address + demo.qr_payload,
    );
  });

  it("finds nothing in a picture without a code", () => {
    const blank = new Uint8ClampedArray(200 * 200 * 4).fill(255);
    expect(readQrCode({ data: blank, width: 200, height: 200 })).toBeNull();
  });
});

describe("the payload of the text of a code", () => {
  it("takes what follows the hash sign of an address", () => {
    expect(tripPayloadOfQr("https://example.org/btt/#QABC")).toBe("QABC");
  });

  it("accepts a code that holds only the payload", () => {
    expect(tripPayloadOfQr("Q0123ABC$*+-./:")).toBe("Q0123ABC$*+-./:");
  });

  it("refuses a code that is something else", () => {
    expect(tripPayloadOfQr("https://example.org/page")).toBeNull();
    expect(tripPayloadOfQr("WIFI:S:home;T:WPA;P:secret;;")).toBeNull();
    expect(tripPayloadOfQr("https://example.org/#")).toBeNull();
    expect(tripPayloadOfQr("")).toBeNull();
  });
});
