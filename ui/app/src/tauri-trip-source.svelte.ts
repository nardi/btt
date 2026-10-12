// Editable trip source backed by Tauri commands and a planning event channel.

import { commands } from "@btt/shared/bindings";
import type {
  CommandError,
  EuroRange,
  GeoCoordinates,
  LegPlan,
  NearbyDaySuggestion,
  PassScenarioLeg,
  PlaceSearchResult,
  PlanningRunId,
  PlanningSessionEvent,
  Trip,
  TripEdit,
  TripEditNotice,
  TripEditOutcome,
  TripHistoryState,
  TripId,
  TicketLeg,
  TripPlaceId,
} from "@btt/shared/bindings";
import { itineraryRows, plannedArrivalDate, tripPlaces } from "@btt/shared/itinerary";
import { journeyKey, liveCandidates } from "@btt/shared/live-ranking";
import {
  scenarioLeg,
  selectedScenario,
  type PassScenarioState,
} from "@btt/shared/scenarios";
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
  /** What undo and redo would do. A newly opened trip has no history. */
  history: TripHistoryState = $state({ undo_description: null, redo_description: null });
  passScenarios: PassScenarioState | null = $state(null);
  readonly livePricesByJourney = new SvelteMap<string, EuroRange>();
  /** Latest planning run. Events of older runs are stale and get dropped. */
  private latestRunId: PlanningRunId = -1;
  /** Counts requests for pass scenarios, so an answer to an older one gets dropped. */
  private scenarioRequestNumber = 0;
  /** Journeys waiting for a live price lookup, in the order the legs were planned. */
  private liveQueue: { key: string; leg: TicketLeg }[] = [];
  /** Keys of the journeys whose live price was looked up or is queued, so none repeats. */
  private readonly liveKeysSeen = new Set<string>();
  /** Whether a loop works through `liveQueue`. */
  private liveLookupRunning = false;
  /** Set when the trip closes, which stops the live lookups. */
  private closed = false;

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
    this.closed = true;
    await commands.closeTrip(this.trip.id);
  }

  async applyEdit(edit: TripEdit): Promise<void> {
    this.acceptEditResult(await commands.applyTripEdit(this.trip.id, edit));
  }

  async addPlaceAt(coordinates: GeoCoordinates): Promise<void> {
    this.acceptEditResult(await commands.addPlaceAtCoordinates(this.trip.id, coordinates));
  }

  async undo(): Promise<void> {
    this.acceptHistoryStep(await commands.undoTripEdit(this.trip.id));
  }

  async redo(): Promise<void> {
    this.acceptHistoryStep(await commands.redoTripEdit(this.trip.id));
  }

  /** Takes the trip from an undo or redo, which returns nothing when its stack is empty. */
  private acceptHistoryStep(
    result:
      | { status: "ok"; data: TripEditOutcome | null }
      | { status: "error"; error: CommandError },
  ): void {
    if (result.status === "ok" && result.data === null) {
      return;
    }
    this.acceptEditResult(
      result.status === "ok" ? { status: "ok", data: result.data! } : result,
    );
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
    this.history = result.data.history;
    this.forgetRemovedLegs();
  }

  selectScenario(index: number): void {
    if (this.passScenarios !== null) {
      this.passScenarios = { ...this.passScenarios, selectedIndex: index };
    }
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
      if (result.Found !== undefined) {
        this.queueLiveLookups(arrival_place_id, result.Found);
      }
    } else if (event.NearbyDaySuggested !== undefined) {
      const { run_id, arrival_place_id, suggestion } = event.NearbyDaySuggested;
      if (run_id !== this.latestRunId) {
        return;
      }
      this.nearbyDaysByArrivalPlace.set(arrival_place_id, suggestion);
    } else if (event.PlanningFinished !== undefined) {
      if (event.PlanningFinished.run_id === this.latestRunId) {
        void this.refreshPassScenarios();
      }
    }
  }

  /**
   * Queues live price lookups for the candidates of a leg, when the traveller weighs prices and
   * allows live prices. The lookups run one after the other in the background, because each
   * takes seconds and the booking sites space their requests.
   */
  private queueLiveLookups(arrivalPlaceId: TripPlaceId, legPlan: LegPlan): void {
    const preferences = this.trip.preferences;
    if (preferences.price_weight === 0 || !preferences.ticket_preferences.live_prices) {
      return;
    }
    for (const journey of liveCandidates(legPlan)) {
      const key = journeyKey(journey);
      if (this.liveKeysSeen.has(key)) {
        continue;
      }
      this.liveKeysSeen.add(key);
      this.liveQueue.push({
        key,
        leg: { arrival_place_id: arrivalPlaceId, journey, stations: legPlan.station_table },
      });
    }
    if (!this.liveLookupRunning) {
      void this.runLiveLookups();
    }
  }

  /** Looks up the queued journeys until the queue is empty or the trip closes. */
  private async runLiveLookups(): Promise<void> {
    this.liveLookupRunning = true;
    try {
      for (let next = this.liveQueue.shift(); next !== undefined && !this.closed; next = this.liveQueue.shift()) {
        const result = await commands.liveJourneyPrice(this.trip.id, next.leg);
        if (result.status === "ok" && result.data !== null) {
          this.livePricesByJourney.set(next.key, result.data);
        }
      }
    } finally {
      this.liveLookupRunning = false;
    }
  }

  /**
   * Asks the backend for the pass scenarios of the planned legs. Without a price weight a pass
   * saves nothing that counts, and without pass options there is nothing to choose, so the
   * trip then has none. The pick survives when a scenario with the same pass comes back.
   */
  private async refreshPassScenarios(): Promise<void> {
    this.scenarioRequestNumber += 1;
    const requestNumber = this.scenarioRequestNumber;
    const plannedArrivalDates = new Map<TripPlaceId, string>();
    for (const [placeId, state] of this.legStatesByArrivalPlace) {
      if (state.status === "found") {
        plannedArrivalDates.set(placeId, plannedArrivalDate(state.legPlan));
      }
    }
    const legs: PassScenarioLeg[] = [];
    const arrivalPlaceIds: TripPlaceId[] = [];
    for (const leg of itineraryRows(this.trip, plannedArrivalDates).legs) {
      const state = this.legStatesByArrivalPlace.get(leg.arrivalPlace.id);
      if (state?.status === "found") {
        legs.push(scenarioLeg(state.legPlan, leg.departureDate));
        arrivalPlaceIds.push(leg.arrivalPlace.id);
      }
    }
    if (this.trip.preferences.price_weight === 0 || !legs.some((leg) => leg.pass_options.length > 0)) {
      this.passScenarios = null;
      return;
    }
    const result = await commands.planPassScenarios(this.trip.id, legs);
    if (requestNumber !== this.scenarioRequestNumber) {
      return;
    }
    if (result.status === "error") {
      this.errorMessage = commandErrorMessage(result.error);
      return;
    }
    // Without an earlier pick the lowest total, which comes first, is the default.
    const picked = selectedScenario(this.passScenarios);
    const kept =
      picked === null
        ? 0
        : result.data.findIndex((scenario) => (scenario.pass?.name ?? null) === (picked.pass?.name ?? null));
    this.passScenarios = {
      scenarios: result.data,
      arrivalPlaceIds,
      selectedIndex: Math.max(kept, 0),
    };
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
