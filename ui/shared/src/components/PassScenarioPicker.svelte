<script lang="ts">
  // The choice between single tickets and the best passes for the whole trip. The legs show the
  // journeys of the picked scenario.
  import { scenarioSummary, scenarioTitle, type PassScenarioState } from "../scenarios";

  let {
    scenarioState,
    onSelect,
  }: {
    scenarioState: PassScenarioState;
    /** Picks a scenario by position. Missing in a read-only view. */
    onSelect?: (index: number) => void;
  } = $props();
</script>

<fieldset class="scenarios">
  <legend>Tickets for the trip</legend>
  {#each scenarioState.scenarios as scenario, index (index)}
    <label class="scenario">
      <input
        type="radio"
        name="pass-scenario"
        checked={index === scenarioState.selectedIndex}
        disabled={onSelect === undefined}
        onchange={() => onSelect?.(index)}
      />
      <span class="title">{scenarioTitle(scenario)}</span>
      <span class="summary">
        {scenarioSummary(scenario)}{index === 0 ? ", the best by your price weight" : ""}
      </span>
    </label>
  {/each}
  <p class="note">The prices are estimates. The Tickets screen prices the picked journeys.</p>
</fieldset>

<style>
  .scenarios {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin: 0;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .scenario {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: var(--space-2);
    align-items: baseline;
  }

  .summary {
    grid-column: 2;
    color: var(--color-text-muted);
  }

  .note {
    margin: 0;
    color: var(--color-text-muted);
  }
</style>
