// Gives the rides of a snapshot the stations they pass, so the map draws their lines along the
// tracks.

import type { Journey, LegPlan, SnapshotLeg, TrainRide, TripSnapshot } from "@btt/shared/bindings";
import { type RailGraph, ridePathPoints } from "./rail-graph";

/** Returns a ride with `path_coordinates` set to the stations its train most likely passes. */
function rideWithPath(plan: LegPlan, ride: TrainRide, graph: RailGraph): TrainRide {
  const boarding = plan.station_table[ride.boarding_station];
  const alighting = plan.station_table[ride.alighting_station];
  if (boarding === undefined || alighting === undefined) {
    return ride;
  }
  const points = ridePathPoints(graph, boarding.coordinates, alighting.coordinates, ride.train_label.category);
  return points.length === 0 ? ride : { ...ride, path_coordinates: points };
}

/** Returns a journey with the stations that each of its rides passes. */
function journeyWithPaths(plan: LegPlan, journey: Journey, graph: RailGraph): Journey {
  return {
    ...journey,
    first_ride: rideWithPath(plan, journey.first_ride, graph),
    later_rides: journey.later_rides.map((later) => ({ ...later, ride: rideWithPath(plan, later.ride, graph) })),
  };
}

/**
 * Returns the snapshot with the stations that the rides of every chosen journey pass. The map
 * draws only the chosen journeys, so the alternatives stay as they are. A ride whose stations the
 * graph does not know, or whose path is not close to the straight line, keeps no path, and the
 * map draws it straight.
 */
export function withRidePaths(snapshot: TripSnapshot, graph: RailGraph): TripSnapshot {
  const legs: SnapshotLeg[] = snapshot.legs.map((leg) => {
    const plan = leg.result.Found;
    if (plan === undefined) {
      return leg;
    }
    return {
      ...leg,
      result: { Found: { ...plan, chosen_journey: journeyWithPaths(plan, plan.chosen_journey, graph) } },
    };
  });
  return { ...snapshot, legs };
}
