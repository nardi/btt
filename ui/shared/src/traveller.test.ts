import { describe, expect, it } from "vitest";

import { ageGroupName, ageOn, blankTraveller } from "./traveller";

describe("ageOn", () => {
  it("counts a year on the birthday", () => {
    expect(ageOn("1998-06-15", "2026-06-14")).toBe(27);
    expect(ageOn("1998-06-15", "2026-06-15")).toBe(28);
  });

  it("never goes below zero", () => {
    expect(ageOn("2030-01-01", "2026-01-01")).toBe(0);
  });
});

describe("ageGroupName", () => {
  it("matches the Interrail age groups", () => {
    expect(ageGroupName(3)).toBe("infant, travels free");
    expect(ageGroupName(4)).toBe("child");
    expect(ageGroupName(12)).toBe("youth");
    expect(ageGroupName(28)).toBe("adult");
    expect(ageGroupName(60)).toBe("senior");
  });
});

describe("blankTraveller", () => {
  it("starts in second class with no cards", () => {
    const traveller = blankTraveller(new Date(2026, 5, 1));
    expect(traveller.birth_date).toBe("1996-01-01");
    expect(traveller.discount_cards).toEqual([]);
    expect(traveller.owned_pass).toBeNull();
  });
});
