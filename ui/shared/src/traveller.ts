// Helpers for traveller profiles on the setup screen.

import type { DiscountCard, Traveller } from "./bindings";

/** Countries a traveller can live in, as two-letter codes with their names. */
export const RESIDENCE_COUNTRIES: readonly { code: string; name: string }[] = [
  { code: "AT", name: "Austria" },
  { code: "BE", name: "Belgium" },
  { code: "BG", name: "Bulgaria" },
  { code: "HR", name: "Croatia" },
  { code: "CZ", name: "Czechia" },
  { code: "DK", name: "Denmark" },
  { code: "EE", name: "Estonia" },
  { code: "FI", name: "Finland" },
  { code: "FR", name: "France" },
  { code: "DE", name: "Germany" },
  { code: "GR", name: "Greece" },
  { code: "HU", name: "Hungary" },
  { code: "IE", name: "Ireland" },
  { code: "IT", name: "Italy" },
  { code: "LV", name: "Latvia" },
  { code: "LT", name: "Lithuania" },
  { code: "LU", name: "Luxembourg" },
  { code: "NL", name: "Netherlands" },
  { code: "NO", name: "Norway" },
  { code: "PL", name: "Poland" },
  { code: "PT", name: "Portugal" },
  { code: "RO", name: "Romania" },
  { code: "RS", name: "Serbia" },
  { code: "SK", name: "Slovakia" },
  { code: "SI", name: "Slovenia" },
  { code: "ES", name: "Spain" },
  { code: "SE", name: "Sweden" },
  { code: "CH", name: "Switzerland" },
  { code: "GB", name: "United Kingdom" },
];

/** Names of the discount cards, in the order the setup screen lists them. */
export const DISCOUNT_CARDS: readonly { card: DiscountCard; name: string }[] = [
  { card: "BahnCard25", name: "BahnCard 25" },
  { card: "BahnCard50", name: "BahnCard 50" },
  { card: "BahnCard100", name: "BahnCard 100" },
  { card: "SwissHalfFare", name: "Swiss Half Fare Card" },
  { card: "OebbVorteilscard", name: "ÖBB Vorteilscard" },
  { card: "SncfAvantage", name: "SNCF Carte Avantage" },
];

/** Creates a second class traveller born 30 years before `today`, with no cards or pass. */
export function blankTraveller(today: Date = new Date()): Traveller {
  const birthYear = today.getFullYear() - 30;
  return {
    id: crypto.randomUUID(),
    name: "Traveller",
    birth_date: `${birthYear}-01-01`,
    residence_country: "NL",
    travel_class: "Second",
    discount_cards: [],
    owned_pass: null,
    accompanying_traveller: null,
  };
}

/** Returns the age in whole years on a day, both given as `YYYY-MM-DD`. */
export function ageOn(birthDate: string, day: string): number {
  const [birthYear, birthMonth, birthDay] = birthDate.split("-").map(Number);
  const [year, month, dayOfMonth] = day.split("-").map(Number);
  let age = year! - birthYear!;
  if (month! < birthMonth! || (month === birthMonth && dayOfMonth! < birthDay!)) {
    age -= 1;
  }
  return Math.max(0, age);
}

/** Names the Interrail age group for an age, as the Rust side does. */
export function ageGroupName(age: number): string {
  if (age < 4) return "infant, travels free";
  if (age < 12) return "child";
  if (age < 28) return "youth";
  if (age < 60) return "adult";
  return "senior";
}
