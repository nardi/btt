<script lang="ts">
  // One leg between two places: its planned trains and whether it includes a night train.
  import type {
    AcceptedBerths,
    Journey,
    LegPlan,
    LegTravelMode,
    NoJourneyReason,
    OvernightLegOptions,
    TripEdit,
  } from "../bindings";
  import { formatDate, formatDuration, formatTimeInZone } from "../format";
  import type { LegRow } from "../itinerary";
  import type { LegViewState } from "../trip-source";
  import TimeWindowField from "./TimeWindowField.svelte";

  let {
    leg,
    legState,
    onEdit,
  }: {
    leg: LegRow;
    legState: LegViewState;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
  } = $props();

  /** Options a leg gets when the user first turns on the night train. */
  const defaultOvernightOptions: OvernightLegOptions = {
    boarding_window: { start: 19 * 60, length: 5 * 60 },
    alighting_window: { start: 6 * 60, length: 4 * 60 },
    accepted_berths: { Either: { preferred: null } },
    minimum_time_on_night_train: null,
  };

  const overnightOptions = $derived(
    leg.travelMode === "Daytime" ? null : leg.travelMode.Overnight,
  );

  const noJourneyMessages: Record<NoJourneyReason, string> = {
    NoStationNearDeparturePlace: "No station with service near the departure place.",
    NoStationNearArrivalPlace: "No station with service near the arrival place.",
    NoConnectionInWindow: "No connection fits the time windows.",
    BackendUnavailable: "The planner could not be reached.",
    TravelDateOutsideTimetable: "The timetable does not cover this date yet.",
  };

  function setTravelMode(travelMode: LegTravelMode) {
    onEdit?.({ SetLegTravelMode: { arrival_place_id: leg.arrivalPlace.id, travel_mode: travelMode } });
  }

  function setOvernightOptions(changes: Partial<OvernightLegOptions>) {
    if (overnightOptions !== null) {
      setTravelMode({ Overnight: { ...overnightOptions, ...changes } });
    }
  }

  /** Maps the berth select's value to accepted berths. */
  function acceptedBerthsFromChoice(choice: string): AcceptedBerths {
    switch (choice) {
      case "Couchette":
        return { Only: "Couchette" };
      case "Sleeper":
        return { Only: "Sleeper" };
      default:
        return { Either: { preferred: null } };
    }
  }

  function berthChoice(acceptedBerths: AcceptedBerths): string {
    return acceptedBerths.Only ?? "Either";
  }

  /** Returns door-to-door minutes, including the ways to and from the stations. */
  function journeyMinutes(journey: Journey): number {
    const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
    return (
      journey.access_duration +
      (lastRide.arrival_time - journey.first_ride.departure_time) +
      (journey.arrival_footpath?.duration ?? 0) +
      journey.egress_duration
    );
  }

  function stationName(legPlan: LegPlan, stationIndex: number): string {
    return legPlan.station_table[stationIndex]?.name ?? "Unknown station";
  }

  function stationTime(legPlan: LegPlan, stationIndex: number, moment: number): string {
    const timeZone = legPlan.station_table[stationIndex]?.time_zone ?? "UTC";
    return formatTimeInZone(moment, timeZone);
  }
</script>

<li class="leg-row" class:overnight={overnightOptions !== null}>
  <div class="header">
    <span class="date">{formatDate(leg.departureDate)}</span>
    {#if onEdit !== undefined}
      <label class="night-train-toggle">
        <input
          type="checkbox"
          checked={overnightOptions !== null}
          onchange={(event) =>
            setTravelMode(
              event.currentTarget.checked ? { Overnight: defaultOvernightOptions } : "Daytime",
            )}
        />
        <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
          <path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z" fill="currentColor" />
        </svg>
        Night train
      </label>
    {/if}
  </div>

  {#if overnightOptions !== null}
    <div class="overnight-options">
      <TimeWindowField
        label="Board between"
        window={overnightOptions.boarding_window}
        disabled={onEdit === undefined}
        onChange={(boarding_window) => setOvernightOptions({ boarding_window })}
      />
      <TimeWindowField
        label="Leave between"
        window={overnightOptions.alighting_window}
        disabled={onEdit === undefined}
        onChange={(alighting_window) => setOvernightOptions({ alighting_window })}
      />
      <label>
        Berth
        <select
          value={berthChoice(overnightOptions.accepted_berths)}
          disabled={onEdit === undefined}
          onchange={(event) =>
            setOvernightOptions({ accepted_berths: acceptedBerthsFromChoice(event.currentTarget.value) })}
        >
          <option value="Either">Couchette or sleeper</option>
          <option value="Couchette">Couchette only</option>
          <option value="Sleeper">Sleeper only</option>
        </select>
      </label>
    </div>
  {/if}

  <div class="plan">
    {#if legState.status === "pending"}
      <span class="pending">Planning…</span>
    {:else if legState.status === "none"}
      <span class="no-journey">{noJourneyMessages[legState.reason]}</span>
    {:else}
      {@const legPlan = legState.legPlan}
      {@const journey = legPlan.chosen_journey}
      <ol class="rides">
        <li class="station-way">
          {formatDuration(journey.access_duration)} to
          {stationName(legPlan, journey.first_ride.boarding_station)}
        </li>
        {#each [journey.first_ride, ...journey.later_rides.map((later) => later.ride)] as ride, rideIndex (rideIndex)}
          <li>
            <span class="time">{stationTime(legPlan, ride.boarding_station, ride.departure_time)}</span>
            {stationName(legPlan, ride.boarding_station)}
            <span class="train">{ride.train_label.category} {ride.train_label.number}</span>
            <span class="time">{stationTime(legPlan, ride.alighting_station, ride.arrival_time)}</span>
            {stationName(legPlan, ride.alighting_station)}
          </li>
        {/each}
        {#if journey.arrival_footpath !== null}
          <li class="station-way">
            {formatDuration(journey.arrival_footpath.duration)} on to
            {stationName(legPlan, journey.arrival_footpath.to_station)}
          </li>
        {/if}
        <li class="station-way">
          {formatDuration(journey.egress_duration)} to {leg.arrivalPlace.name}
        </li>
      </ol>
      <span class="summary">
        {formatDuration(journeyMinutes(journey))}
        · {journey.later_rides.length === 0
          ? "direct"
          : `${journey.later_rides.length} ${journey.later_rides.length === 1 ? "transfer" : "transfers"}`}
        {#if legPlan.alternative_journeys.length > 0}
          · {legPlan.alternative_journeys.length} alternatives
        {/if}
      </span>
    {/if}
  </div>
</li>

<style>
  .leg-row {
    list-style: none;
    margin-left: calc(0.9em + var(--space-3));
    padding: var(--space-2) var(--space-4);
    border-left: 3px solid var(--color-border);
  }

  .leg-row.overnight {
    border-left-style: dashed;
    border-left-color: var(--color-accent);
  }

  .header {
    display: flex;
    align-items: center;
    gap: var(--space-3);
  }

  .date {
    color: var(--color-text-muted);
  }

  .night-train-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }

  .overnight-options {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: var(--space-2) 0;
  }

  .rides {
    margin: var(--space-1) 0;
    padding: 0;
    list-style: none;
  }

  .time {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }

  .train {
    margin: 0 var(--space-2);
    padding: 0 var(--space-1);
    border-radius: 4px;
    background: var(--color-surface-muted);
  }

  .pending,
  .summary,
  .station-way {
    color: var(--color-text-muted);
  }

  .no-journey {
    color: var(--color-danger);
  }
</style>
