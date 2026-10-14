// The read-only trip source of the viewer, built from a snapshot.

import type { LegViewState, TripSource } from "@btt/shared/trip-source";
import type { TripPlaceId, TripSnapshot } from "@btt/shared/bindings";

/**
 * Returns the source that shows a snapshot. Every leg of the snapshot is in its final state,
 * so nothing is ever pending. A leg that the snapshot has no entry for was not planned when the
 * traveller shared the trip, which the viewer shows as a leg without a journey.
 */
export function snapshotTripSource(snapshot: TripSnapshot): TripSource {
  const legStates = new Map<TripPlaceId, LegViewState>();
  for (const leg of snapshot.legs) {
    const { Found: legPlan, NoJourneyFound: reason } = leg.result;
    if (legPlan !== undefined) {
      legStates.set(leg.arrival_place_id, { status: "found", legPlan });
    } else if (reason !== undefined) {
      legStates.set(leg.arrival_place_id, { status: "none", reason });
    }
  }
  for (const placeId of arrivalPlaceIds(snapshot)) {
    if (!legStates.has(placeId)) {
      legStates.set(placeId, { status: "none", reason: "BackendUnavailable" });
    }
  }
  return {
    kind: "readonly",
    viewState: {
      trip: snapshot.trip,
      legStatesByArrivalPlace: legStates,
      nearbyDaysByArrivalPlace: new Map(),
      notices: [],
      errorMessage: null,
      passScenarios: null,
      livePricesByJourney: new Map(),
      extrasPendingArrivalPlaces: new Set(),
    },
  };
}

/** Returns the arrival place of every leg of the trip. */
function arrivalPlaceIds(snapshot: TripSnapshot): TripPlaceId[] {
  const itinerary = snapshot.trip.itinerary;
  if (itinerary === "Empty" || "OriginOnly" in itinerary) {
    return [];
  }
  return [...itinerary.Route.via_places.map((via) => via.place.id), itinerary.Route.destination.id];
}
