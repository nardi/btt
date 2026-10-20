<script lang="ts">
  // One leg between two places. The closed card shows the journey at a glance, and the open card
  // shows its rides and the settings of a night train.
  import type {
    AcceptedBerths,
    EuroRange,
    Journey,
    JourneyChoice,
    LegPlan,
    LegTravelMode,
    NearbyDaySuggestion,
    NoJourneyReason,
    OvernightLegOptions,
    TripEdit,
  } from "../bindings";
  import { alternativeRows, describeKindsForTooltip } from "../alternatives";
  import {
    explainStationWay,
    formatDate,
    formatDuration,
    formatMinutesOfDay,
    formatStationWay,
    parseMinutesOfDay,
  } from "../format";
  import {
    journeyOverview,
    journeyRides,
    type LegLine,
    type LegRow,
    stationName,
    stationTime,
    trainName,
    transferSummary,
    transferWay,
  } from "../itinerary";
  import { journeyKey } from "../live-ranking";
  import { expandTransition, rowEnter, rowExit } from "../motion";
  import { describeJourneyPrice, formatJourneyPrice } from "../tickets";
  import type { LegViewState } from "../trip-source";
  import AlternativesPopup from "./AlternativesPopup.svelte";
  import Icon from "./Icon.svelte";
  import JourneyExplanation from "./JourneyExplanation.svelte";
  import JourneyLine from "./JourneyLine.svelte";
  import Spinner from "./Spinner.svelte";
  import TrainChip from "./TrainChip.svelte";
  import type { Snippet } from "svelte";
  import { fly, slide } from "svelte/transition";

  let {
    leg,
    legState,
    legLine,
    storedChoice,
    storedTransferTime,
    defaultTransferTime,
    nearbyDay,
    livePrices,
    onEdit,
    onAddStop,
    stopSearch,
    enterDelay,
  }: {
    leg: LegRow;
    legState: LegViewState;
    /** The colour the leg takes on the itinerary spine and its train chips. */
    legLine: LegLine;
    /** The journey the traveller picked for this leg, or `null` for the default pick. */
    storedChoice: JourneyChoice | null;
    /** Minutes the traveller set as the minimum transfer time of this leg, or `null` for the trip's. */
    storedTransferTime: number | null;
    /** Minimum transfer time of the whole trip in minutes, which the leg uses without its own. */
    defaultTransferTime: number;
    /** A much better day before or after the leg's date, or `null`. */
    nearbyDay: NearbyDaySuggestion | null;
    /** Live prices of journeys from the booking sites, by `journeyKey`. */
    livePrices: ReadonlyMap<string, EuroRange>;
    /** Missing in a read-only view. */
    onEdit?: (edit: TripEdit) => void;
    /** Opens or closes the search for a stop on this leg. Missing in a read-only view. */
    onAddStop?: () => void;
    /** The search for the stop being added, which exists while the search is open. */
    stopSearch?: Snippet;
    /** Staggers the enter transition of the rows of a freshly opened trip. */
    enterDelay: number;
  } = $props();

  /** Whether the rides and the settings of the leg are open. */
  let expanded = $state(false);

  /** Whether the explanation of the chosen journey is open. */
  let showExplanation = $state(false);

  const plan: LegPlan | null = $derived(legState.status === "found" ? legState.legPlan : null);
  const journey: Journey | null = $derived(plan?.chosen_journey ?? null);
  const rides = $derived(journey === null ? [] : journeyRides(journey));
  const overnightOptions = $derived(leg.travelMode === "Daytime" ? null : leg.travelMode.Overnight);
  const rows = $derived(plan === null ? [] : alternativeRows(plan, nearbyDay));
  const price = $derived(journey?.cost_parts?.price ?? null);
  const live = $derived(journey !== null && livePrices.has(journeyKey(journey)));
  const priceText = $derived(formatJourneyPrice(price, live));
  const sleepingRide = $derived(
    journey?.sleeping_ride === null || journey?.sleeping_ride === undefined
      ? null
      : (rides[journey.sleeping_ride] ?? null),
  );
  // A snapshot carries no cost parts, so the explanation exists only in the editable view.
  const hasExplanation = $derived(
    onEdit !== undefined && journey?.cost_parts !== null && journey?.cost_parts !== undefined,
  );
  const canExpand = $derived(plan !== null || overnightOptions !== null);

  /**
   * Returns whether a journey is the one a choice describes. A plan whose alternatives still
   * hold the picked journey is from before the pick and about to be replaced, so the leg row
   * doesn't claim the pick is gone.
   */
  function matchesChoice(candidate: Journey, choice: JourneyChoice): boolean {
    const candidateRides = journeyRides(candidate);
    return (
      candidate.first_ride.departure_time === choice.first_departure_time &&
      candidateRides.length === choice.trains.length &&
      candidateRides.every(
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

  /** Makes a journey the chosen one for this leg, or goes back to the default pick. */
  function chooseJourney(picked: Journey | null) {
    const choice =
      picked === null
        ? null
        : {
            first_departure_time: picked.first_ride.departure_time,
            trains: journeyRides(picked).map((ride) => ride.train_label),
          };
    onEdit?.({ ChooseJourney: { arrival_place_id: leg.arrivalPlace.id, choice } });
  }

  /** Sets the minimum transfer time of this leg, or goes back to the trip's time with `null`. */
  function setTransferTime(minutes: number | null) {
    onEdit?.({
      SetLegMinimumTransferTime: { arrival_place_id: leg.arrivalPlace.id, minimum_transfer_time: minutes },
    });
  }

  /** Sets the transfer time from the input, ignoring text that is not a number of minutes. */
  function setTransferTimeFromInput(text: string) {
    const minutes = Number.parseInt(text, 10);
    if (Number.isFinite(minutes)) {
      setTransferTime(Math.min(120, Math.max(0, minutes)));
    }
  }

  /** Options a leg gets when the user first turns on the night train. */
  const defaultOvernightOptions: OvernightLegOptions = {
    bedtime: 23 * 60,
    wake_up_time: 6 * 60 + 30,
    accepted_berths: { Either: { preferred: null } },
  };

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

  /** Returns the trains of a journey, such as "RE 2". */
  function journeyTrains(candidate: Journey): string {
    return journeyRides(candidate).map(trainName).join(", ");
  }

  /** Returns the price of a journey for a note, or a placeholder when it has none. */
  function notePrice(candidate: Journey): string {
    return formatJourneyPrice(candidate.cost_parts?.price ?? null, false) ?? "an unknown price";
  }
</script>

<li
  class="leg-row"
  style:--leg-colour={legLine.colour}
  in:fly={rowEnter(enterDelay)}
  out:fly={rowExit()}
>
  <div class="spine" aria-hidden="true">
    <div class="line leg" class:night={legLine.night}></div>
  </div>
  <div class="leg-card" class:expanded data-popup-edge>
    <div class="head">
      <!-- An open card names its trains in the list of rides, so the header keeps only the date. -->
      {#if !expanded && journey !== null}
        <span class="chips">
          {#each rides as ride, rideIndex (rideIndex)}
            <TrainChip
              label={trainName(ride)}
              colour={legLine.colour}
              sleeping={journey.sleeping_ride === rideIndex}
            />
          {/each}
        </span>
      {/if}
      <span class="when numbers" class:open={expanded}>{formatDate(leg.departureDate)}</span>
      <span class="grow"></span>
      {#if hasExplanation}
        <button
          type="button"
          class="icon-button"
          aria-expanded={showExplanation}
          aria-label="Why this journey"
          title="Why this journey"
          onclick={() => (showExplanation = !showExplanation)}
        >
          ?
        </button>
      {/if}
      {#if onAddStop !== undefined}
        <button
          type="button"
          class="icon-button"
          class:on={stopSearch !== undefined}
          aria-pressed={stopSearch !== undefined}
          aria-label="Add a stop on this leg"
          title="Add a stop on this leg"
          onclick={onAddStop}
        >
          <Icon name="plus" />
        </button>
      {/if}
      {#if onEdit !== undefined}
        <button
          type="button"
          class="icon-button moon"
          class:on={overnightOptions !== null}
          aria-pressed={overnightOptions !== null}
          aria-label="Take a night train"
          title="Take a night train"
          onclick={() =>
            setTravelMode(overnightOptions === null ? { Overnight: defaultOvernightOptions } : "Daytime")}
        >
          <Icon name="moon" />
        </button>
      {/if}
      {#if canExpand}
        <button
          type="button"
          class="icon-button"
          aria-expanded={expanded}
          aria-label={expanded ? "Hide details" : "Show details"}
          title={expanded ? "Hide details" : "Show details"}
          onclick={() => (expanded = !expanded)}
        >
          <Icon name="chevron" turned={expanded} />
        </button>
      {/if}
    </div>

    {#if stopSearch !== undefined}
      <div class="stop-search" transition:slide={expandTransition()}>
        {@render stopSearch()}
      </div>
    {/if}

    {#if legState.status === "pending"}
      <span class="pending">
        <Spinner label="Planning the leg" />
        Planning…
      </span>
    {:else if legState.status === "none"}
      <span class="no-journey">{noJourneyMessages[legState.reason]}</span>
    {:else if plan !== null && journey !== null}
      <!-- Opening the card swaps the overview for the list of rides, which says the same in
           more detail. -->
      {#if expanded}
        <ol class="rides">
          <li class="station-way" title={explainStationWay(journey.access)}>
            {formatStationWay(journey.access, stationName(plan, journey.first_ride.boarding_station))}
          </li>
          {#each rides as ride, rideIndex (rideIndex)}
            <!-- A change between two stations has a way that the rides alone do not show. -->
            {@const changeWay = rideIndex === 0 ? null : transferWay(plan, rides[rideIndex - 1]!, ride)}
            {#if changeWay !== null}
              <li class="station-way" title={explainStationWay(changeWay)}>
                {formatStationWay(changeWay, stationName(plan, ride.boarding_station))}
              </li>
            {/if}
            <li>
              <span class="time numbers">{stationTime(plan, ride.boarding_station, ride.departure_time)}</span>
              <span class="station">{stationName(plan, ride.boarding_station)}</span>
              <TrainChip
                label={trainName(ride)}
                colour={legLine.colour}
                sleeping={journey.sleeping_ride === rideIndex}
                small
              />
              <span class="time numbers">{stationTime(plan, ride.alighting_station, ride.arrival_time)}</span>
              <span class="station">{stationName(plan, ride.alighting_station)}</span>
            </li>
          {/each}
          {#if journey.arrival_footpath !== null}
            <li class="station-way" title={explainStationWay(journey.arrival_footpath.way)}>
              {formatStationWay(
                journey.arrival_footpath.way,
                stationName(plan, journey.arrival_footpath.to_station),
              )}
            </li>
          {/if}
          <li class="station-way" title={explainStationWay(journey.egress)}>
            {formatStationWay(journey.egress, leg.arrivalPlace.name)}
          </li>
        </ol>
      {:else}
        {@const overview = journeyOverview(plan, journey)}
        <JourneyLine rideMinutes={rides.map((ride) => ride.arrival_time - ride.departure_time)} night={legLine.night} />
        <div class="times">
          <span>
            <span class="time numbers">{overview.boarding.time}</span>
            <span class="station">{overview.boarding.station}</span>
          </span>
          <span>
            <span class="time numbers">{overview.alighting.time}</span>
            <span class="station">{overview.alighting.station}</span>
          </span>
          <span class="duration numbers">
            {formatDuration(overview.minutes)}, {transferSummary(journey)}
          </span>
        </div>
      {/if}

      {#if legState.status === "found" && legState.extrasPending === true}
        <!-- The searches after the main plan take longer than the plan itself. -->
        <span class="note">
          <Spinner label="Searching for cheaper and faster journeys and pass options" />
          Looking for cheaper and faster journeys…
        </span>
      {/if}
      {#if legState.status === "found" && legState.liveOvertaken !== null && legState.liveOvertaken !== undefined}
        <!-- The booking sites' prices put another journey ahead of the planner's estimates. -->
        <span class="note">
          With the live prices, {journeyTrains(journey)} at {notePrice(journey)} now comes before
          {journeyTrains(legState.liveOvertaken)}, which the estimates put first.
        </span>
      {/if}
      {#if overnightOptions !== null && journey.sleeping_ride === null}
        <span class="night-warning">
          No night train with berths covers most of the night from
          {formatMinutesOfDay(overnightOptions.bedtime)} to
          {formatMinutesOfDay(overnightOptions.wake_up_time)} on this date.
        </span>
      {/if}
      {#if pickIsGone(plan)}
        <span class="night-warning">
          The journey you picked is no longer possible, so this is the default pick.
          {#if onEdit !== undefined}
            <button type="button" class="small" onclick={() => chooseJourney(null)}>
              Use the default pick
            </button>
          {/if}
        </span>
      {/if}

      {#if priceText !== null || sleepingRide !== null || plan.follows_traveller_choice || rows.length > 0}
        <!-- The price slot sits first, so the price of the planner has its place before a leg has
             one. The row keeps its height without a price. -->
        <div class="meta">
          <span class="price" title={price === null ? undefined : describeJourneyPrice(price, live)}>
            {#if priceText !== null}
              {priceText}
              <span class="dot" class:open={!live}></span>
            {/if}
          </span>
          {#if sleepingRide !== null}
            <span class="note sleeping"><span>Sleeping on {trainName(sleepingRide)}</span></span>
          {/if}
          {#if plan.follows_traveller_choice}
            <span class="note">
              <span>Your pick</span>
              {#if onEdit !== undefined}
                <button
                  type="button"
                  class="icon-button"
                  aria-label="Use the default pick"
                  title="Use the default pick"
                  onclick={() => chooseJourney(null)}
                >
                  <Icon name="close" />
                </button>
              {/if}
            </span>
          {/if}
          {#if rows.length > 0}
            <AlternativesPopup
              {rows}
              chosen={journey}
              kindsTooltip={describeKindsForTooltip(plan)}
              {livePrices}
              onTake={onEdit === undefined ? undefined : chooseJourney}
            />
          {/if}
        </div>
      {/if}
      {#if showExplanation}
        <JourneyExplanation legPlan={plan} {nearbyDay} />
      {/if}
    {/if}

    {#if expanded && (onEdit !== undefined || storedTransferTime !== null)}
      <!-- Every leg can change how much time it needs between two trains. -->
      <div class="settings" transition:slide={expandTransition()}>
        <h4>Transfers</h4>
        <label>
          <span class="label">Minimum transfer</span>
          <input
            type="number"
            class="numbers"
            min="0"
            max="120"
            value={storedTransferTime ?? defaultTransferTime}
            disabled={onEdit === undefined}
            onchange={(event) => setTransferTimeFromInput(event.currentTarget.value)}
          />
          <span class="hint">minutes</span>
          {#if storedTransferTime !== null && onEdit !== undefined}
            <button type="button" class="small" onclick={() => setTransferTime(null)}>
              Use default
            </button>
          {/if}
        </label>
      </div>
    {/if}

    {#if expanded && overnightOptions !== null}
      <!-- The settings of the night train sit in their own panel, apart from the journey and
           its properties. -->
      <div class="settings" transition:slide={expandTransition()}>
        <h4>Night train settings</h4>
        <label>
          <span class="label">In bed by</span>
          <input
            type="time"
            class="numbers"
            value={formatMinutesOfDay(overnightOptions.bedtime)}
            disabled={onEdit === undefined}
            onchange={(event) => setOvernightTime("bedtime", event.currentTarget.value)}
          />
        </label>
        <label>
          <span class="label">Up at</span>
          <input
            type="time"
            class="numbers"
            value={formatMinutesOfDay(overnightOptions.wake_up_time)}
            disabled={onEdit === undefined}
            onchange={(event) => setOvernightTime("wake_up_time", event.currentTarget.value)}
          />
        </label>
        <label>
          <span class="label">Berth</span>
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
  </div>
</li>

<style>
  /* The row is a grid: the spine cell on the left, the leg card 12px away from it. */
  .leg-row {
    display: grid;
    grid-template-columns: 34px 1fr;
    column-gap: var(--space-3);
    list-style: none;
  }

  .spine {
    position: relative;
  }

  .line.leg {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    transform: translateX(-50%);
    width: 4px;
    border-radius: var(--radius-pill);
    background: var(--leg-colour);
  }

  /* A night leg draws dashed, in the purple that marks night trains everywhere. */
  .line.leg.night {
    background: repeating-linear-gradient(
      to bottom,
      var(--leg-colour) 0 8px,
      transparent 8px 13px
    );
  }

  .leg-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-3);
    padding: var(--space-3) var(--space-4);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  .grow {
    flex: 1;
  }

  .when {
    color: var(--color-text-muted);
    font-size: var(--text-caption);
  }

  /* An open card has no chips, so its date reads in the normal ink. */
  .when.open {
    color: var(--color-text);
    font-size: var(--text-small);
    font-weight: 600;
  }

  /* The moon is filled with the night colour while the leg takes a night train. */
  .moon.on {
    color: var(--color-night);
    border-color: var(--color-night);
    background: color-mix(in srgb, var(--color-night) 12%, var(--color-surface));
  }

  /* The plus takes the accent colour while the search for a stop is open. */
  .icon-button.on:not(.moon) {
    color: var(--color-accent);
    border-color: var(--color-accent);
    background: color-mix(in srgb, var(--color-accent) 12%, var(--color-surface));
  }

  .times {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-1) 14px;
  }

  .time {
    font-weight: 700;
  }

  .station {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .duration {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .pending {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) 2px;
    color: var(--color-text-muted);
  }

  .no-journey {
    color: var(--color-danger);
  }

  .rides {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .rides .station {
    font-size: inherit;
  }

  .station-way {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  /* A note below the journey, such as work that is still going on or a changed order. */
  .note {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .night-warning {
    color: var(--color-danger);
    font-size: var(--text-small);
  }

  /* The properties of the journey sit under a hairline. The row keeps its height and its left
     slot without a price. */
  .meta {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-3);
    min-height: 44px;
    padding-top: var(--space-2);
    border-top: 1px solid var(--color-border);
  }

  .price {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-weight: 600;
  }

  .sleeping {
    color: var(--color-night);
  }

  /* The settings of a leg, such as its night train, in a tinted panel of their own. */
  .settings {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-1);
    padding: var(--space-3);
    border-radius: 10px;
    background: var(--color-surface-muted);
  }

  .settings h4 {
    margin: 0;
    color: var(--color-text-muted);
    font-size: var(--text-small);
    font-weight: 600;
  }

  .settings label {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .label {
    min-width: 7em;
    color: var(--color-text-muted);
  }

  .hint {
    color: var(--color-text-muted);
  }
</style>
