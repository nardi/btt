// The interface through which shared components read and edit a trip.
//
// The desktop app implements it with Tauri commands, and the web viewer will implement the
// read-only variant from a snapshot. Components never import Tauri APIs themselves.

import type {
  EuroRange,
  GeoCoordinates,
  Journey,
  LegPlan,
  NearbyDaySuggestion,
  NoJourneyReason,
  PlaceSearchResult,
  Trip,
  TripEdit,
  TripEditNotice,
  TripHistoryState,
  TripPlaceId,
} from "./bindings";
import { plannedArrivalDate } from "./itinerary";
import { legPlanWithLivePrices } from "./live-ranking";
import { legPlanOnPass, passOptionOfLeg, type PassScenarioState } from "./scenarios";

/** What the frontend knows about the plan of one leg. */
export type LegViewState =
  | { status: "pending" }
  | {
      status: "found";
      legPlan: LegPlan;
      /** The journey the estimates put first, when a live price moved another ahead of it. */
      liveOvertaken?: Journey | null;
      /**
       * Whether the planner is still searching for the cheaper and faster journey and the pass
       * options of the leg, which the plan lacks until it finishes.
       */
      extrasPending?: boolean;
    }
  | { status: "none"; reason: NoJourneyReason };

/** The trip and everything the views show about it. */
export interface TripViewState {
  readonly trip: Trip;
  /** Plan state per leg, keyed by the leg's arrival place. A missing leg is pending. */
  readonly legStatesByArrivalPlace: ReadonlyMap<TripPlaceId, LegViewState>;
  /** A much better day before or after, per leg, keyed by the leg's arrival place. */
  readonly nearbyDaysByArrivalPlace: ReadonlyMap<TripPlaceId, NearbyDaySuggestion>;
  /** Side effects of the last edit, such as a dropped overnight option. */
  readonly notices: readonly TripEditNotice[];
  /** Message of the last failed edit or search, or `null`. */
  readonly errorMessage: string | null;
  /** Single tickets and the best passes for the trip, or `null` when no pass is worth a look. */
  readonly passScenarios: PassScenarioState | null;
  /** Live prices of journeys from the booking sites, by `journeyKey`. */
  readonly livePricesByJourney: ReadonlyMap<string, EuroRange>;
  /** Legs, by arrival place, whose plan still waits for the searches after the main plan. */
  readonly extrasPendingArrivalPlaces: ReadonlySet<TripPlaceId>;
}

/** A source of a trip. Only the editable variant has functions that change it. */
export type TripSource =
  | {
      readonly kind: "editable";
      readonly viewState: TripViewState;
      applyEdit(edit: TripEdit): Promise<void>;
      searchPlaces(query: string): Promise<PlaceSearchResult[]>;
      /** What undo and redo would do. */
      readonly history: TripHistoryState;
      /** Takes back the most recent edit. */
      undo(): Promise<void>;
      /** Applies the most recently undone edit again. */
      redo(): Promise<void>;
      /** Adds a place at a position picked on the map. */
      addPlaceAt(coordinates: GeoCoordinates): Promise<void>;
      /** Shows the journeys of another pass scenario, by position. */
      selectScenario(index: number): void;
    }
  | {
      readonly kind: "readonly";
      readonly viewState: TripViewState;
    };

/**
 * Returns the date each planned leg arrives on, by arrival place. The journeys of the picked
 * pass scenario decide the dates, like they decide the legs.
 */
export function plannedArrivalDates(viewState: TripViewState): Map<TripPlaceId, string> {
  const dates = new Map<TripPlaceId, string>();
  for (const placeId of viewState.legStatesByArrivalPlace.keys()) {
    const legState = legViewState(viewState, placeId);
    if (legState.status === "found") {
      dates.set(placeId, plannedArrivalDate(legState.legPlan));
    }
  }
  return dates;
}

/**
 * Returns the plan state of a leg, which is pending until the planner reports. A leg that rides
 * on a pass day in the picked scenario shows the journey of its pass option. Otherwise the live
 * prices the app knows replace the estimates of its journeys, which can change the first one.
 */
export function legViewState(viewState: TripViewState, arrivalPlaceId: TripPlaceId): LegViewState {
  const state = viewState.legStatesByArrivalPlace.get(arrivalPlaceId) ?? { status: "pending" };
  if (state.status !== "found") {
    return state;
  }
  const extrasPending = viewState.extrasPendingArrivalPlaces.has(arrivalPlaceId);
  const option = passOptionOfLeg(viewState.passScenarios, arrivalPlaceId);
  if (option !== null) {
    return { status: "found", legPlan: legPlanOnPass(state.legPlan, option), extrasPending };
  }
  if (viewState.livePricesByJourney.size === 0) {
    return { ...state, extrasPending };
  }
  const live = legPlanWithLivePrices(
    state.legPlan,
    viewState.livePricesByJourney,
    viewState.trip.preferences.price_weight,
  );
  return { status: "found", legPlan: live.legPlan, liveOvertaken: live.overtaken, extrasPending };
}
