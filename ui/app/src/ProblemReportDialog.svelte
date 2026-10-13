<script lang="ts">
  // Saves a problem report for the open trip: the trip, its planned legs, what the planner
  // considered, the map view and recent log lines, with a note from the traveller.
  import { commands, type GeoCoordinates, type TripId } from "@btt/shared/bindings";

  import { commandErrorMessage } from "./tauri-trip-source.svelte";

  let {
    tripId,
    selectedPlaceId,
    mapView,
    onClose,
  }: {
    tripId: TripId;
    selectedPlaceId: number | null;
    /** Where the map looks, or `null` before it first moved. */
    mapView: { center: GeoCoordinates; zoom: number } | null;
    onClose: () => void;
  } = $props();

  let note = $state("");
  let saving = $state(false);
  let message: string | null = $state(null);

  async function save() {
    saving = true;
    message = null;
    const result = await commands.exportProblemReport(tripId, note, {
      selected_place_id: selectedPlaceId,
      map_center: mapView?.center ?? { latitude: 0, longitude: 0 },
      map_zoom: mapView?.zoom ?? 0,
    });
    saving = false;
    if (result.status === "error") {
      message = commandErrorMessage(result.error);
    } else if (result.data !== null) {
      message = `Saved to ${result.data}`;
    }
  }
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="problem-report-title">
    <h2 id="problem-report-title">Report a problem</h2>
    <p>
      Describe what looks wrong, such as an odd route, a missing train or a ticket price that is
      far off. The report file holds this trip with its places, the planned journeys and what the
      planner considered, and the ticket plan with the live price lookups and the answers of the
      booking sites, so the problem can be reproduced. Share it only with people you want to see your trip.
    </p>
    <textarea bind:value={note} rows="5" placeholder="What did you expect, and what did you see?"
    ></textarea>
    {#if message !== null}
      <p class="message">{message}</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onClose}>Close</button>
      <button type="button" class="primary" disabled={saving} onclick={save}>
        {saving ? "Saving…" : "Save report…"}
      </button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 35%);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    width: min(520px, calc(100vw - 2 * var(--space-4)));
    padding: var(--space-4) var(--space-6);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  h2 {
    margin: 0;
    font-weight: 700;
    font-size: var(--text-heading);
  }

  p {
    margin: 0;
  }

  textarea {
    font: inherit;
    padding: var(--space-2);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
    color: inherit;
    resize: vertical;
  }

  .message {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
