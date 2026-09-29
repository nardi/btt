// Editable trip source backed by Tauri commands and a planning event channel.

import { commands } from "@btt/shared/bindings";
import type {
  CommandError,
  GeoCoordinates,
  NearbyDaySuggestion,
  PlaceSearchResult,
  PlanningRunId,
  PlanningSessionEvent,
  Trip,
  TripEdit,
  TripEditNotice,
  TripEditOutcome,
  TripId,
  TripPlaceId,
} from "@btt/shared/bindings";
import { tripPlaces } from "@btt/shared/itinerary";
import type { LegViewState, TripViewState } from "@btt/shared/trip-source";
import { Channel } from "@tauri-apps/api/core";
import { SvelteMap } from "svelte/reactivity";

/** Turns a command error into a message for the user. */
export function commandErrorMessage(error: CommandError): string {
  if (error.TripNotFound !== undefined) {
    return "This trip no longer exists.";
  }
  if (error.TripNotOpen !== undefined) {
    return "This trip is not open. Reopen it from the trip list.";
  }
  if (error.InvalidEdit !== undefined) {
    return "That change does not fit the itinerary.";
  }
  if (error.Storage !== undefined) {
    return `Saving failed: ${error.Storage.message}`;
  }
  return `Place search failed: ${error.PlaceSearch.message}`;
}

/** An open trip that the desktop app edits and plans through the Rust backend. */
export class TauriTripSource implements TripViewState {
  readonly kind = "editable";
  trip: Trip = $state()!;
  readonly legStatesByArrivalPlace = new SvelteMap<TripPlaceId, LegViewState>();
  readonly nearbyDaysByArrivalPlace = new SvelteMap<TripPlaceId, NearbyDaySuggestion>();
  notices: TripEditNotice[] = $state([]);
  errorMessage: string | null = $state(null);
  /** Latest planning run. Events of older runs are stale and get dropped. */
  private latestRunId: PlanningRunId = -1;

  private constructor(trip: Trip) {
    this.trip = trip;
  }

  get viewState(): TripViewState {
    return this;
  }

  /** Opens a trip and starts receiving its planning events. */
  static async open(tripId: TripId): Promise<TauriTripSource | CommandError> {
    const events = new Channel<PlanningSessionEvent>();
    let source: TauriTripSource | null = null;
    // Events can arrive before `openTrip` returns, so they wait until the source exists.
    const earlyEvents: PlanningSessionEvent[] = [];
    events.onmessage = (event) => {
      if (source === null) {
        earlyEvents.push(event);
      } else {
        source.handleEvent(event);
      }
    };
    const result = await commands.openTrip(tripId, events);
    if (result.status === "error") {
      return result.error;
    }
    source = new TauriTripSource(result.data);
    for (const event of earlyEvents) {
      source.handleEvent(event);
    }
    return source;
  }

  /** Stops planning. The source must not be used afterwards. */
  async close(): Promise<void> {
    await commands.closeTrip(this.trip.id);
  }

  async applyEdit(edit: TripEdit): Promise<void> {
    this.acceptEditResult(await commands.applyTripEdit(this.trip.id, edit));
  }

  async addPlaceAt(coordinates: GeoCoordinates): Promise<void> {
    this.acceptEditResult(await commands.addPlaceAtCoordinates(this.trip.id, coordinates));
  }

  /** Takes the trip and notices from an edit command, or shows its error. */
  private acceptEditResult(
    result: { status: "ok"; data: TripEditOutcome } | { status: "error"; error: CommandError },
  ): void {
    if (result.status === "error") {
      this.errorMessage = commandErrorMessage(result.error);
      return;
    }
    this.errorMessage = null;
    this.trip = result.data.trip;
    this.notices = result.data.notices;
    this.forgetRemovedLegs();
  }

  async searchPlaces(query: string): Promise<PlaceSearchResult[]> {
    const result = await commands.searchPlaces(query);
    if (result.status === "error") {
      this.errorMessage = commandErrorMessage(result.error);
      return [];
    }
    return result.data;
  }

  /** Applies one planning event to the leg states. */
  private handleEvent(event: PlanningSessionEvent): void {
    if (event.TripChanged !== undefined) {
      this.trip = event.TripChanged;
      this.forgetRemovedLegs();
    } else if (event.PlanningStarted !== undefined) {
      if (event.PlanningStarted.run_id < this.latestRunId) {
        return;
      }
      this.latestRunId = event.PlanningStarted.run_id;
      for (const placeId of event.PlanningStarted.replanned_arrival_place_ids) {
        this.legStatesByArrivalPlace.set(placeId, { status: "pending" });
        this.nearbyDaysByArrivalPlace.delete(placeId);
      }
    } else if (event.LegPlanned !== undefined) {
      const { run_id, arrival_place_id, result } = event.LegPlanned;
      if (run_id !== this.latestRunId) {
        return;
      }
      this.legStatesByArrivalPlace.set(
        arrival_place_id,
        result.Found !== undefined
          ? { status: "found", legPlan: result.Found }
          : { status: "none", reason: result.NoJourneyFound },
      );
    } else if (event.NearbyDaySuggested !== undefined) {
      const { run_id, arrival_place_id, suggestion } = event.NearbyDaySuggested;
      if (run_id !== this.latestRunId) {
        return;
      }
      this.nearbyDaysByArrivalPlace.set(arrival_place_id, suggestion);
    }
  }

  /** Drops leg states of places the trip no longer has. */
  private forgetRemovedLegs(): void {
    const placeIds = new Set(tripPlaces(this.trip.itinerary).map((place) => place.id));
    for (const placeId of this.legStatesByArrivalPlace.keys()) {
      if (!placeIds.has(placeId)) {
        this.legStatesByArrivalPlace.delete(placeId);
        this.nearbyDaysByArrivalPlace.delete(placeId);
      }
    }
  }
}
