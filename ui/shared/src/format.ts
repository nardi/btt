// Formatting of times, dates and durations for display.

import type { DailyTimeWindow, UnixTimeMinutes } from "./bindings";

const minutesPerDay = 1440;

/** Formats minutes since local midnight as `HH:MM`. Values past midnight wrap to the next day. */
export function formatMinutesOfDay(minutesSinceMidnight: number): string {
  const wrapped = ((minutesSinceMidnight % minutesPerDay) + minutesPerDay) % minutesPerDay;
  const hours = Math.floor(wrapped / 60);
  const minutes = wrapped % 60;
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}`;
}

/** Parses `HH:MM` into minutes since midnight, or returns `null` when the text is not a time. */
export function parseMinutesOfDay(text: string): number | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(text.trim());
  if (match === null) {
    return null;
  }
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  if (hours > 23 || minutes > 59) {
    return null;
  }
  return hours * 60 + minutes;
}

/** Formats a daily window as its local start and end, such as `22:00–01:00`. */
export function formatDailyTimeWindow(window: DailyTimeWindow): string {
  if (window.length >= minutesPerDay) {
    return "any time";
  }
  return `${formatMinutesOfDay(window.start)}–${formatMinutesOfDay(window.start + window.length)}`;
}

/**
 * Builds a daily window from its local start and end in minutes since midnight. An end before
 * the start crosses midnight, and an end equal to the start covers the whole day.
 *
 * Follows `DailyTimeWindow::from_start_and_end` in Rust.
 */
export function dailyTimeWindow(startMinutes: number, endMinutes: number): DailyTimeWindow {
  const length =
    endMinutes > startMinutes ? endMinutes - startMinutes : minutesPerDay - startMinutes + endMinutes;
  return { start: startMinutes, length };
}

/** Formats a length of time, such as `7 h 28 min` or `45 min`. */
export function formatDuration(totalMinutes: number): string {
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) {
    return `${minutes} min`;
  }
  return minutes === 0 ? `${hours} h` : `${hours} h ${minutes} min`;
}

/** Formats a moment as `HH:MM` on the local clock of a time zone, such as a station's. */
export function formatTimeInZone(moment: UnixTimeMinutes, timeZone: string): string {
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    timeZone,
  }).format(new Date(moment * 60_000));
}

/** Formats a `YYYY-MM-DD` date with its weekday, such as `Thu 1 Oct`. */
export function formatDate(isoDate: string): string {
  return new Intl.DateTimeFormat("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
    timeZone: "UTC",
  }).format(new Date(`${isoDate}T00:00:00Z`));
}

/** Formats a distance, such as `800 m`, `2.5 km` or `12 km`. */
export function formatDistance(meters: number): string {
  if (meters < 1000) {
    return `${Math.round(meters)} m`;
  }
  const kilometers = meters / 1000;
  return kilometers < 10 ? `${Number(kilometers.toFixed(1))} km` : `${Math.round(kilometers)} km`;
}
