// The interface through which shared components read and edit a trip.
//
// The desktop app implements it with Tauri commands, and the web viewer will implement the
// read-only variant from a snapshot. Components never import Tauri APIs themselves.

import type {
  GeoCoordinates,
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
import { legPlanOnPass, passOptionOfLeg, type PassScenarioState } from "./scenarios";

/** What the frontend knows about the plan of one leg. */
export type LegViewState =
  | { status: "pending" }
  | { status: "found"; legPlan: LegPlan }
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
 * Returns the plan state of a leg, which is pending until the planner reports. A leg that rides
 * on a pass day in the picked scenario shows the journey of its pass option.
 */
export function legViewState(viewState: TripViewState, arrivalPlaceId: TripPlaceId): LegViewState {
  const state = viewState.legStatesByArrivalPlace.get(arrivalPlaceId) ?? { status: "pending" };
  const option = passOptionOfLeg(viewState.passScenarios, arrivalPlaceId);
  if (state.status !== "found" || option === null) {
    return state;
  }
  return { status: "found", legPlan: legPlanOnPass(state.legPlan, option) };
}
