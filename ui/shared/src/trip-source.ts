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
  TripPlaceId,
} from "./bindings";

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
}

/** A source of a trip. Only the editable variant has functions that change it. */
export type TripSource =
  | {
      readonly kind: "editable";
      readonly viewState: TripViewState;
      applyEdit(edit: TripEdit): Promise<void>;
      searchPlaces(query: string): Promise<PlaceSearchResult[]>;
      /** Adds a place at a position picked on the map. */
      addPlaceAt(coordinates: GeoCoordinates): Promise<void>;
    }
  | {
      readonly kind: "readonly";
      readonly viewState: TripViewState;
    };

/** Returns the plan state of a leg, which is pending until the planner reports. */
export function legViewState(viewState: TripViewState, arrivalPlaceId: TripPlaceId): LegViewState {
  return viewState.legStatesByArrivalPlace.get(arrivalPlaceId) ?? { status: "pending" };
}
