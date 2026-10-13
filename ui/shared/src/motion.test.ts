import { describe, expect, it } from "vitest";

import { staggerDelay } from "./motion";

describe("staggerDelay", () => {
  it("delays each row by 40 ms", () => {
    expect(staggerDelay(0)).toBe(0);
    expect(staggerDelay(1)).toBe(40);
    expect(staggerDelay(3)).toBe(120);
  });

  it("stops at 200 ms so a long trip does not animate for long", () => {
    expect(staggerDelay(5)).toBe(200);
    expect(staggerDelay(40)).toBe(200);
  });

  it("never delays by a negative time", () => {
    expect(staggerDelay(-1)).toBe(0);
  });
});
