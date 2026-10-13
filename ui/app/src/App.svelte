<script lang="ts">
  // The desktop app: a list of trips, or the itinerary of the open trip.
  import {
    commands,
    type GeoCoordinates,
    type Traveller,
    type TravellerId,
    type LiveQuote,
    type TicketItem,
    type TicketLeg,
    type TicketPlan,
    type TripId,
    type TripSummary,
  } from "@btt/shared/bindings";
  import ItineraryView from "@btt/shared/components/ItineraryView.svelte";
  import SetupView from "@btt/shared/components/SetupView.svelte";
  import TicketsView from "@btt/shared/components/TicketsView.svelte";
  import TripMap from "@btt/shared/components/TripMap.svelte";
  import Icon from "@btt/shared/components/Icon.svelte";
  import { itineraryRows, legLine, type LegLine } from "@btt/shared/itinerary";
  import { liveQuoteKey } from "@btt/shared/tickets";
  import { legViewState } from "@btt/shared/trip-source";
  import type { PlaceFocusRequest } from "@btt/shared/map-features";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { onMount } from "svelte";

  import { commandErrorMessage, TauriTripSource } from "./tauri-trip-source.svelte";
  import ProblemReportDialog from "./ProblemReportDialog.svelte";
  import TripList from "./TripList.svelte";

  let trips: TripSummary[] = $state([]);
  let openSource: TauriTripSource | null = $state(null);
  let errorMessage: string | null = $state(null);
  let selectedPlaceId: number | null = $state(null);
  let focusRequest: PlaceFocusRequest | null = $state(null);
  let showProblemReport = $state(false);
  /** The screens of an open trip. A trip opens on planning. */
  type Screen = "setup" | "planning" | "tickets";
  let screen: Screen = $state("planning");
  const screens: { screen: Screen; label: string; enabled: boolean }[] = [
    { screen: "setup", label: "Setup", enabled: true },
    { screen: "planning", label: "Planning", enabled: true },
    { screen: "tickets", label: "Tickets", enabled: true },
  ];
  let travellers: Traveller[] = $state([]);
  /** The tickets of the open trip, as the backend last priced them. */
  let ticketPlan: TicketPlan | null = $state(null);
  let ticketsLoading = $state(false);
  /** Legs of the open trip that have no journey, which the ticket plan leaves out. */
  let unplannedLegs = $state(0);
  /** Live prices from booking sites by ticket, with `null` for a ticket the site gave none for. */
  let livePrices: Map<string, LiveQuote | null> = $state(new Map());
  let liveLoading = $state(false);
  /** The keys of the tickets whose live price is being looked up again. */
  let retrying: ReadonlySet<string> = $state(new Set());
  /** The color of each leg of the open trip by its arrival place, as the itinerary draws it. */
  const legLines: ReadonlyMap<number, LegLine> = $derived.by(() => {
    const source = openSource;
    if (source === null) {
      return new Map();
    }
    return new Map(
      itineraryRows(source.trip).legs.map(
        (leg, index) => [leg.arrivalPlace.id, legLine(leg.travelMode, index + 1)] as const,
      ),
    );
  });
  /** Numbers the pricing requests, so that a late answer to an old request is dropped. */
  let ticketRequestNumber = 0;
  /** Where the map of the open trip looks, for problem reports. */
  let mapView: { center: GeoCoordinates; zoom: number } | null = $state(null);

  async function refreshTrips() {
    const result = await commands.listTrips();
    if (result.status === "ok") {
      trips = result.data;
    } else {
      errorMessage = commandErrorMessage(result.error);
    }
  }

  async function refreshTravellers() {
    const result = await commands.listTravellers();
    if (result.status === "ok") {
      travellers = result.data;
    } else {
      errorMessage = commandErrorMessage(result.error);
    }
  }

  /** Saves a traveller profile and shows it in the list at once. */
  async function saveTraveller(traveller: Traveller) {
    const known = travellers.some((existing) => existing.id === traveller.id);
    travellers = known
      ? travellers.map((existing) => (existing.id === traveller.id ? traveller : existing))
      : [...travellers, traveller];
    const result = await commands.saveTraveller(traveller);
    if (result.status === "error") {
      errorMessage = commandErrorMessage(result.error);
      await refreshTravellers();
    }
  }

  async function deleteTraveller(travellerId: TravellerId) {
    const result = await commands.deleteTraveller(travellerId);
    if (result.status === "error") {
      errorMessage = commandErrorMessage(result.error);
    }
    await refreshTravellers();
  }

  async function openTrip(tripId: TripId) {
    const opened = await TauriTripSource.open(tripId);
    if (opened instanceof TauriTripSource) {
      openSource = opened;
      screen = "planning";
      errorMessage = null;
    } else {
      errorMessage = commandErrorMessage(opened);
    }
  }

  async function createTrip(name: string, startDate: string) {
    const result = await commands.createTrip(name, startDate);
    if (result.status === "ok") {
      await openTrip(result.data.id);
    } else {
      errorMessage = commandErrorMessage(result.error);
    }
  }

  async function deleteTrip(tripId: TripId) {
    const result = await commands.deleteTrip(tripId);
    if (result.status === "error") {
      errorMessage = commandErrorMessage(result.error);
    }
    await refreshTrips();
  }

  async function closeTrip() {
    await openSource?.close();
    openSource = null;
    selectedPlaceId = null;
    focusRequest = null;
    await refreshTrips();
  }

  async function renameTrip(name: string) {
    const trimmed = name.trim();
    if (openSource !== null && trimmed !== "" && trimmed !== openSource.trip.name) {
      await openSource.applyEdit({ RenameTrip: trimmed });
    }
  }

  /** Selects a place and asks the map to move to it. */
  function focusPlace(placeId: number) {
    selectedPlaceId = placeId;
    focusRequest = { placeId, requestNumber: (focusRequest?.requestNumber ?? 0) + 1 };
  }

  /**
   * Undoes on Ctrl+Z and redoes on Ctrl+Shift+Z or Ctrl+Y. A text field keeps its own undo for
   * typing, so the shortcuts only act when the focus is elsewhere.
   */
  function handleHistoryShortcut(event: KeyboardEvent) {
    if (openSource === null || !(event.ctrlKey || event.metaKey) || event.altKey) {
      return;
    }
    const target = event.target;
    const isTextField =
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement ||
      (target instanceof HTMLElement && target.isContentEditable);
    if (isTextField) {
      return;
    }
    const key = event.key.toLowerCase();
    if (key === "z" && !event.shiftKey) {
      event.preventDefault();
      void openSource.undo();
    } else if ((key === "z" && event.shiftKey) || key === "y") {
      event.preventDefault();
      void openSource.redo();
    }
  }

  /** Prices the trip whenever the tickets screen is open and the journeys or travellers change. */
  $effect(() => {
    const source = openSource;
    if (screen !== "tickets" || source === null) {
      return;
    }
    // The plan depends on the saved travellers, so a change to them prices the trip again.
    void travellers;
    const legs: TicketLeg[] = [];
    let missing = 0;
    for (const leg of itineraryRows(source.trip).legs) {
      // A leg on a pass day of the picked scenario is priced with its pass journey.
      const state = legViewState(source, leg.arrivalPlace.id);
      if (state?.status === "found") {
        legs.push({
          arrival_place_id: leg.arrivalPlace.id,
          journey: state.legPlan.chosen_journey,
          stations: state.legPlan.station_table,
        });
      } else {
        missing += 1;
      }
    }
    unplannedLegs = missing;
    ticketsLoading = true;
    ticketRequestNumber += 1;
    const requestNumber = ticketRequestNumber;
    void commands.planTickets(source.trip.id, legs).then((result) => {
      if (requestNumber !== ticketRequestNumber) {
        return;
      }
      ticketsLoading = false;
      if (result.status === "ok") {
        ticketPlan = result.data;
        void fetchLivePrices(result.data, requestNumber);
      } else {
        errorMessage = commandErrorMessage(result.error);
      }
    });
  });

  /**
   * Looks up the live price of each ticket that has a booking site to ask, one after the other.
   * The backend keeps the answers and spaces its requests, so a repeat costs nothing.
   */
  async function fetchLivePrices(plan: TicketPlan, requestNumber: number) {
    const requests = new Map<string, NonNullable<(typeof plan.strategies)[number]["items"][number]["live_quote_request"]>>();
    for (const strategy of plan.strategies) {
      for (const item of strategy.items) {
        const key = liveQuoteKey(item);
        if (key !== null && item.live_quote_request !== null && !livePrices.has(key)) {
          requests.set(key, item.live_quote_request);
        }
      }
    }
    if (requests.size === 0) {
      return;
    }
    liveLoading = true;
    for (const [key, request] of requests) {
      if (requestNumber !== ticketRequestNumber) {
        break;
      }
      const result = await commands.fetchLiveQuote(request);
      const next = new Map(livePrices);
      next.set(key, result.status === "ok" ? result.data : null);
      livePrices = next;
    }
    if (requestNumber === ticketRequestNumber) {
      liveLoading = false;
    }
  }

  /**
   * Looks up the live price of one ticket again. The backend keeps a site's answer for half an
   * hour, so the request asks it to ask the site anew.
   */
  async function retryLivePrice(item: TicketItem) {
    const key = liveQuoteKey(item);
    const request = item.live_quote_request;
    if (key === null || request === null || retrying.has(key)) {
      return;
    }
    retrying = new Set(retrying).add(key);
    const requestNumber = ticketRequestNumber;
    const result = await commands.fetchLiveQuote({ ...request, refresh: true });
    if (requestNumber === ticketRequestNumber) {
      const next = new Map(livePrices);
      next.set(key, result.status === "ok" ? result.data : null);
      livePrices = next;
    }
    const remaining = new Set(retrying);
    remaining.delete(key);
    retrying = remaining;
  }

  /** Opens a booking link in the system browser. */
  async function openBookingLink(url: string) {
    try {
      await openUrl(url);
    } catch {
      errorMessage = "The link could not be opened.";
    }
  }

  onMount(() => {
    void refreshTrips();
    void refreshTravellers();
  });
</script>

<svelte:window onkeydown={handleHistoryShortcut} />

<main class:trip-open={openSource !== null}>
  {#if errorMessage !== null}
    <p class="error" role="alert">{errorMessage}</p>
  {/if}

  {#if openSource === null}
    <TripList {trips} onCreate={createTrip} onOpen={openTrip} onDelete={deleteTrip} />
  {:else}
    <header class="trip-header">
      <button type="button" class="with-icon" onclick={closeTrip}>
        <Icon name="back" />
        Trips
      </button>
      <input
        class="trip-name"
        type="text"
        value={openSource.trip.name}
        aria-label="Trip name"
        onchange={(event) => renameTrip(event.currentTarget.value)}
      />
      <label class="start-date">
        <span class="label">Start</span>
        <input
          type="date"
          class="numbers"
          value={openSource.trip.start_date}
          onchange={(event) => {
            if (event.currentTarget.value !== "") {
              void openSource?.applyEdit({ SetStartDate: event.currentTarget.value });
            }
          }}
        />
      </label>
      <div class="history-buttons">
        <button
          type="button"
          class="icon-button large"
          disabled={openSource.history.undo_description === null}
          aria-label="Undo"
          title={openSource.history.undo_description === null
            ? "Nothing to undo"
            : `Undo: ${openSource.history.undo_description} (Ctrl+Z)`}
          onclick={() => void openSource?.undo()}
        >
          <Icon name="undo" />
        </button>
        <button
          type="button"
          class="icon-button large"
          disabled={openSource.history.redo_description === null}
          aria-label="Redo"
          title={openSource.history.redo_description === null
            ? "Nothing to redo"
            : `Redo: ${openSource.history.redo_description} (Ctrl+Shift+Z)`}
          onclick={() => void openSource?.redo()}
        >
          <Icon name="redo" />
        </button>
      </div>
      <button type="button" onclick={() => (showProblemReport = true)}>Report a problem</button>
    </header>
    {#if showProblemReport}
      <ProblemReportDialog
        tripId={openSource.trip.id}
        {selectedPlaceId}
        {mapView}
        onClose={() => (showProblemReport = false)}
      />
    {/if}
    <nav class="tabs" aria-label="Screens">
      {#each screens as { screen: tab, label, enabled } (tab)}
        <button
          type="button"
          class:active={screen === tab}
          aria-current={screen === tab ? "page" : undefined}
          disabled={!enabled}
          onclick={() => (screen = tab)}
        >
          {label}
        </button>
      {/each}
    </nav>
    {#key screen}
      <div class="panel">
        {#if screen === "setup"}
          <SetupView
            source={openSource}
            {travellers}
            onSaveTraveller={saveTraveller}
            onDeleteTraveller={deleteTraveller}
          />
        {:else if screen === "tickets"}
          <TicketsView
            plan={ticketPlan}
            {legLines}
            loading={ticketsLoading}
            {livePrices}
            {liveLoading}
            {retrying}
            {unplannedLegs}
            onOpenLink={openBookingLink}
            onRetryLivePrice={retryLivePrice}
          />
        {:else}
          <div class="workspace">
            <div class="itinerary-column">
              <ItineraryView
                source={openSource}
                {selectedPlaceId}
                onSelect={(placeId) => (selectedPlaceId = placeId)}
                onFocus={focusPlace}
                onOpenSetup={() => (screen = "setup")}
              />
            </div>
            <div class="map-column">
              <TripMap
                source={openSource}
                {selectedPlaceId}
                {focusRequest}
                onSelect={(placeId) => (selectedPlaceId = placeId)}
                onViewChange={(view) => (mapView = view)}
              />
            </div>
          </div>
        {/if}
      </div>
    {/key}
  {/if}
</main>

<style>
  main {
    max-width: 880px;
    margin: 0 auto;
    padding: var(--space-6) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    box-sizing: border-box;
  }

  main.trip-open {
    max-width: none;
    height: 100vh;
    padding: var(--space-4);
  }

  .trip-header {
    display: flex;
    align-items: center;
    gap: var(--space-6);
  }

  .trip-name {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: none;
    border-radius: 0;
    background: none;
    font-size: var(--text-title);
    font-weight: 700;
  }

  .trip-name:focus-visible {
    outline-offset: 4px;
  }

  .start-date {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .start-date .label {
    color: var(--color-text-muted);
  }

  .history-buttons {
    display: flex;
    gap: var(--space-1);
  }

  .tabs {
    display: flex;
    gap: var(--space-1);
    border-bottom: 1px solid var(--color-border);
  }

  .tabs button {
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    padding: var(--space-2) var(--space-3) calc(var(--space-2) + 2px);
    color: var(--color-text-muted);
    font-weight: 500;
  }

  .tabs button:hover {
    background: none;
    color: var(--color-text);
  }

  .tabs button.active {
    border-bottom-color: var(--color-accent);
    color: var(--color-text);
    font-weight: 600;
  }

  /* Only the open screen is in the flow. It enters with a short crossfade and slide. */
  .panel {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    animation: panel-enter 150ms ease-out;
  }

  .panel > * {
    flex: 1;
    min-height: 0;
  }

  .workspace {
    display: grid;
    grid-template-columns: minmax(420px, 1fr) minmax(300px, 1.2fr);
    gap: var(--space-4);
    flex: 1;
    min-height: 0;
  }

  .itinerary-column {
    overflow-y: auto;
    padding-right: var(--space-2);
  }

  .map-column {
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .error {
    margin: 0;
    color: var(--color-danger);
  }
</style>
