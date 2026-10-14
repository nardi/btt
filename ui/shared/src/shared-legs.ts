// The legs of a trip as the traveller sees them, which the desktop app sends to the backend to
// share the trip with the web viewer.

import type { SnapshotLeg } from "./bindings";
import { itineraryRows } from "./itinerary";
import { legViewState, type TripViewState } from "./trip-source";

/** The legs to share and how many legs are not ready to share. */
export interface SharedLegs {
  /** The legs that are planned or have no journey, in itinerary order. */
  legs: SnapshotLeg[];
  /**
   * Legs that wait for the planner, either for their plan or for the searches after it. The
   * link holds the plan as it is now, so the traveller shares once these are done.
   */
  unfinishedLegs: number;
}

/**
 * Returns the legs of the open trip as the views show them: the journey of the picked pass
 * scenario or with live prices where those apply.
 */
export function sharedLegs(viewState: TripViewState): SharedLegs {
  const legs: SnapshotLeg[] = [];
  let unfinishedLegs = 0;
  for (const leg of itineraryRows(viewState.trip).legs) {
    const state = legViewState(viewState, leg.arrivalPlace.id);
    if (state.status === "found") {
      legs.push({ arrival_place_id: leg.arrivalPlace.id, result: { Found: state.legPlan } });
      if (state.extrasPending === true) {
        unfinishedLegs += 1;
      }
    } else if (state.status === "none") {
      legs.push({ arrival_place_id: leg.arrivalPlace.id, result: { NoJourneyFound: state.reason } });
    } else {
      unfinishedLegs += 1;
    }
  }
  return { legs, unfinishedLegs };
}
