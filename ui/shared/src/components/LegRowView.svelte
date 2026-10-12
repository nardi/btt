<script lang="ts">
  // One leg between two places: its planned trains and whether it includes a night train.
  import type {
    AcceptedBerths,
    Journey,
    JourneyChoice,
    LegPlan,
    LegTravelMode,
    NearbyDaySuggestion,
    NoJourneyReason,
    OvernightLegOptions,
    TrainRide,
    TripEdit,
  } from "../bindings";
  import {
    explainStationWay,
    formatDate,
    formatDuration,
    formatMinutesOfDay,
    formatStationWay,
    formatTimeInZone,
    parseMinutesOfDay,
  } from "../format";
  import { formatEuroRange, formatEuro } from "../tickets";
  import type { LegRow } from "../itinerary";
  import type { LegViewState } from "../trip-source";
  import JourneyExplanation from "./JourneyExplanation.svelte";

  let {
    leg,
    legState,
    storedChoice,
    nearbyDay,
    onEdit,
  }: {
    leg: LegRow;
    legState: LegViewState;
    /** The journey the traveller picked for this leg, or `null` for the planner's pick. */
    storedChoice: JourneyChoice | null;
    /** A much better day before or after the leg's date, or `null`. */
    nearbyDay: NearbyDaySuggestion | null;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
  } = $props();

  /**
   * Returns whether a journey is the one a choice describes. A plan whose alternatives still
   * hold the picked journey is from before the pick and about to be replaced, so the leg row
   * doesn't claim the pick is gone.
   */
  function matchesChoice(journey: Journey, choice: JourneyChoice): boolean {
    const rides = journeyRides(journey);
    return (
      journey.first_ride.departure_time === choice.first_departure_time &&
      rides.length === choice.trains.length &&
      rides.every(
        (ride, index) =>
          ride.train_label.category === choice.trains[index]?.category &&
          ride.train_label.number === choice.trains[index]?.number,
      )
    );
  }

  /** Returns whether the traveller picked a journey that the latest plan no longer has. */
  function pickIsGone(legPlan: LegPlan): boolean {
    return (
      storedChoice !== null &&
      !legPlan.follows_traveller_choice &&
      !legPlan.alternative_journeys.some((alternative) => matchesChoice(alternative, storedChoice))
    );
  }

  /** Makes a journey the chosen one for this leg, or goes back to the planner's pick. */
  function chooseJourney(journey: Journey | null) {
    const choice =
      journey === null
        ? null
        : {
            first_departure_time: journey.first_ride.departure_time,
            trains: journeyRides(journey).map((ride) => ride.train_label),
          };
    onEdit?.({ ChooseJourney: { arrival_place_id: leg.arrivalPlace.id, choice } });
  }

  /** Options a leg gets when the user first turns on the night train. */
  const defaultOvernightOptions: OvernightLegOptions = {
    bedtime: 23 * 60,
    wake_up_time: 6 * 60 + 30,
    accepted_berths: { Either: { preferred: null } },
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

  /** Sets the bedtime or wake-up time when the input holds a valid time. */
  function setOvernightTime(field: "bedtime" | "wake_up_time", text: string) {
    const minutes = parseMinutesOfDay(text);
    if (minutes !== null) {
      setOvernightOptions({ [field]: minutes });
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

  /** Whether the list of alternative journeys is open. */
  let showAlternatives = $state(false);

  /** Whether the explanation of the chosen journey is open. */
  let showExplanation = $state(false);

  /** Returns the rides of a journey in travel order. */
  function journeyRides(journey: Journey): TrainRide[] {
    return [journey.first_ride, ...journey.later_rides.map((later) => later.ride)];
  }

  /** Describes the number of transfers, such as "direct" or "2 transfers". */
  function transferSummary(journey: Journey): string {
    const count = journey.later_rides.length;
    if (count === 0) {
      return "direct";
    }
    return `${count} ${count === 1 ? "transfer" : "transfers"}`;
  }

  /** Returns door-to-door minutes, including the ways to and from the stations. */
  function journeyMinutes(journey: Journey): number {
    const lastRide = journey.later_rides.at(-1)?.ride ?? journey.first_ride;
    return (
      journey.access.duration +
      (lastRide.arrival_time - journey.first_ride.departure_time) +
      (journey.arrival_footpath?.way.duration ?? 0) +
      journey.egress.duration
    );
  }

  /** Returns the estimated price of the journey's tickets, or `null` when none is known. */
  function journeyPrice(journey: Journey): string | null {
    const price = journey.cost_parts?.price ?? null;
    return price === null ? null : formatEuroRange(price);
  }

  /** Returns the trains of a journey, such as "RE 2". */
  function journeyTrains(journey: Journey): string {
    return journeyRides(journey)
      .map((ride) => `${ride.train_label.category} ${ride.train_label.number}`)
      .join(", ");
  }

  /** Returns the euro cents a candidate journey saves against the chosen one, or `null`. */
  function priceSaving(candidate: Journey, chosen: Journey): number | null {
    const candidatePrice = candidate.cost_parts?.price?.median ?? null;
    const chosenPrice = chosen.cost_parts?.price?.median ?? null;
    if (candidatePrice === null || chosenPrice === null) {
      return null;
    }
    return chosenPrice - candidatePrice;
  }

  /** Returns the minutes a candidate journey takes longer than the chosen one. */
  function minutesLonger(candidate: Journey, chosen: Journey): number {
    return (
      (candidate.cost_parts?.door_to_door_minutes ?? 0) -
      (chosen.cost_parts?.door_to_door_minutes ?? 0)
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
      <label class="time-field">
        <span class="label">In bed by</span>
        <input
          type="time"
          value={formatMinutesOfDay(overnightOptions.bedtime)}
          disabled={onEdit === undefined}
          onchange={(event) => setOvernightTime("bedtime", event.currentTarget.value)}
        />
      </label>
      <label class="time-field">
        <span class="label">Up at</span>
        <input
          type="time"
          value={formatMinutesOfDay(overnightOptions.wake_up_time)}
          disabled={onEdit === undefined}
          onchange={(event) => setOvernightTime("wake_up_time", event.currentTarget.value)}
        />
      </label>
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
        <li class="station-way" title={explainStationWay(journey.access)}>
          {formatStationWay(journey.access, stationName(legPlan, journey.first_ride.boarding_station))}
        </li>
        {#each [journey.first_ride, ...journey.later_rides.map((later) => later.ride)] as ride, rideIndex (rideIndex)}
          <li>
            <span class="time">{stationTime(legPlan, ride.boarding_station, ride.departure_time)}</span>
            {stationName(legPlan, ride.boarding_station)}
            <span class="train" class:sleeping={journey.sleeping_ride === rideIndex}>
              {ride.train_label.category} {ride.train_label.number}
            </span>
            <span class="time">{stationTime(legPlan, ride.alighting_station, ride.arrival_time)}</span>
            {stationName(legPlan, ride.alighting_station)}
          </li>
        {/each}
        {#if journey.arrival_footpath !== null}
          <li class="station-way" title={explainStationWay(journey.arrival_footpath.way)}>
            {formatStationWay(
              journey.arrival_footpath.way,
              stationName(legPlan, journey.arrival_footpath.to_station),
            )}
          </li>
        {/if}
        <li class="station-way" title={explainStationWay(journey.egress)}>
          {formatStationWay(journey.egress, leg.arrivalPlace.name)}
        </li>
      </ol>
      <span class="summary">
        {formatDuration(journeyMinutes(journey))} · {transferSummary(journey)}
        {#if journeyPrice(journey) !== null}
          · {journeyPrice(journey)}
        {/if}
        {#if legPlan.alternative_journeys.length > 0}
          ·
          <button
            type="button"
            class="link"
            aria-expanded={showAlternatives}
            onclick={() => (showAlternatives = !showAlternatives)}
          >
            {legPlan.alternative_journeys.length}
            {legPlan.alternative_journeys.length === 1 ? "alternative" : "alternatives"}
          </button>
        {/if}
        {#if journey.cost_parts !== null && journey.cost_parts !== undefined}
          <button
            type="button"
            class="explain"
            aria-expanded={showExplanation}
            aria-label="Why this journey"
            title="Why this journey"
            onclick={() => (showExplanation = !showExplanation)}
          >
            ?
          </button>
        {/if}
      </span>
      {#if showExplanation}
        <JourneyExplanation {legPlan} {nearbyDay} />
      {/if}
      {#if overnightOptions !== null && journey.sleeping_ride === null}
        <span class="night-note">
          No night train with berths covers most of the night from
          {formatMinutesOfDay(overnightOptions.bedtime)} to
          {formatMinutesOfDay(overnightOptions.wake_up_time)} on this date.
        </span>
      {/if}
      {#if legPlan.follows_traveller_choice}
        <span class="choice-note">
          Your pick ·
          {#if onEdit !== undefined}
            <button type="button" class="link" onclick={() => chooseJourney(null)}>
              use the planner's pick
            </button>
          {/if}
        </span>
      {:else if pickIsGone(legPlan)}
        <span class="choice-note">
          The journey you picked is no longer possible, so this is the planner's pick.
          {#if onEdit !== undefined}
            <button type="button" class="link" onclick={() => chooseJourney(null)}>Forget my pick</button>
          {/if}
        </span>
      {/if}
      {#if showAlternatives}
        <!-- Other good journeys, best first by the same cost that picked the chosen one. -->
        <ol class="alternatives">
          {#each legPlan.alternative_journeys as alternative, alternativeIndex (alternativeIndex)}
            {@const rides = journeyRides(alternative)}
            {@const lastRide = rides.at(-1) ?? alternative.first_ride}
            <li>
              <span class="time">
                {stationTime(legPlan, alternative.first_ride.boarding_station, alternative.first_ride.departure_time)}
              </span>
              {stationName(legPlan, alternative.first_ride.boarding_station)}
              →
              <span class="time">{stationTime(legPlan, lastRide.alighting_station, lastRide.arrival_time)}</span>
              {stationName(legPlan, lastRide.alighting_station)}
              <span class="alternative-details">
                {formatDuration(journeyMinutes(alternative))} · {transferSummary(alternative)} ·
                {rides.map((ride) => `${ride.train_label.category} ${ride.train_label.number}`).join(", ")}
                {#if onEdit !== undefined}
                  · <button type="button" class="link" onclick={() => chooseJourney(alternative)}>
                    Take this one
                  </button>
                {/if}
              </span>
            </li>
          {/each}
        </ol>
      {/if}
      {#if legPlan.cheapest !== null && legPlan.cheapest !== undefined}
        <!-- A journey the planner found by searching for the cheapest, which saves a lot. -->
        {@const cheapest = legPlan.cheapest}
        <span class="nearby-day">
          Cheaper: {journeyTrains(cheapest)}, saves
          {formatEuro(priceSaving(cheapest, journey) ?? 0)} and takes
          {formatDuration(minutesLonger(cheapest, journey))} longer.
          {#if onEdit !== undefined}
            <button type="button" class="link" onclick={() => chooseJourney(cheapest)}>
              Take this one
            </button>
          {/if}
        </span>
      {/if}
      {#if legPlan.fastest !== null && legPlan.fastest !== undefined}
        <!-- A journey the planner found by searching by time alone, which is much faster. -->
        {@const fastest = legPlan.fastest}
        <span class="nearby-day">
          Faster: {journeyTrains(fastest)},
          {formatDuration(-minutesLonger(fastest, journey))} quicker for
          {formatEuro(Math.max(-(priceSaving(fastest, journey) ?? 0), 0))} more.
          {#if onEdit !== undefined}
            <button type="button" class="link" onclick={() => chooseJourney(fastest)}>
              Take this one
            </button>
          {/if}
        </span>
      {/if}
      {#if legState.liveOvertaken !== null && legState.liveOvertaken !== undefined}
        <!-- The booking sites' prices put another journey ahead of the planner's estimates. -->
        <span class="nearby-day">
          With the live prices, {journeyTrains(journey)} at {journeyPrice(journey) ?? "an unknown price"}
          now comes before {journeyTrains(legState.liveOvertaken)}, which the estimates put first.
        </span>
      {/if}
    {/if}
    {#if nearbyDay !== null}
      <!-- The traveller decides how to reach the other day, such as by adding a night. -->
      {@const dayPlan = nearbyDay.plan}
      {@const dayRides = journeyRides(dayPlan.chosen_journey)}
      {@const dayLastRide = dayRides.at(-1) ?? dayPlan.chosen_journey.first_ride}
      <span class="nearby-day">
        {nearbyDay.day_offset < 0 ? "A day earlier" : "A day later"}, {formatDate(nearbyDay.departure_date)}:
        {dayRides.map((ride) => `${ride.train_label.category} ${ride.train_label.number}`).join(", ")},
        {transferSummary(dayPlan.chosen_journey)}, arrives
        {stationTime(dayPlan, dayLastRide.alighting_station, dayLastRide.arrival_time)}.
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

  /* The train the traveller sleeps on, in the purple of night trains on the map. */
  .train.sleeping {
    background: var(--color-night-train);
    color: white;
  }

  .time-field {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .time-field .label {
    min-width: 7em;
    color: var(--color-text-muted);
  }

  .night-note {
    display: block;
    color: var(--color-danger);
  }

  .pending,
  .summary,
  .station-way {
    color: var(--color-text-muted);
  }

  .no-journey {
    color: var(--color-danger);
  }

  /* The alternatives button reads as part of the summary text. */
  /* A small round button, so the explanation stays out of the way until asked for. */
  .explain {
    width: 1.4em;
    height: 1.4em;
    margin-left: var(--space-2);
    padding: 0;
    border: 1px solid var(--color-border);
    border-radius: 50%;
    background: var(--color-surface);
    color: var(--color-text-muted);
    font-size: 0.85em;
    line-height: 1;
  }

  .link {
    padding: 0;
    border: none;
    background: none;
    color: var(--color-accent);
    text-decoration: underline;
  }

  .alternatives {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin: var(--space-2) 0 0;
    padding: 0 0 0 var(--space-3);
    border-left: 2px solid var(--color-border);
    list-style: none;
  }

  .nearby-day {
    display: block;
    margin-top: var(--space-1);
    padding: var(--space-1) var(--space-2);
    border-radius: 4px;
    background: var(--color-notice);
    color: var(--color-text);
  }

  .choice-note {
    display: block;
    color: var(--color-text-muted);
  }

  .alternative-details {
    display: block;
    color: var(--color-text-muted);
  }
</style>
