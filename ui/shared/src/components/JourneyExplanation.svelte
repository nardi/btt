<script lang="ts">
  // Explains why the planner picked a leg's journey, and how each alternative and a nearby
  // day compare with it.
  import type { Journey, JourneyCostParts, LegPlan, NearbyDaySuggestion } from "../bindings";
  import {
    COST_PART_LABELS,
    COST_PART_NAMES,
    describeJourney,
    explainAlternative,
    explainChoice,
    explainNearbyDay,
    explainSearch,
    explainUnits,
    partCosts,
  } from "../explanation";
  import { formatDuration } from "../format";
  import { formatEuroRange } from "../tickets";

  let {
    legPlan,
    nearbyDay,
  }: {
    legPlan: LegPlan;
    /** A much better day before or after, or `null`. */
    nearbyDay: NearbyDaySuggestion | null;
  } = $props();

  /** Formats a part's cost, with a sign when it takes minutes off. */
  function formatCost(minutes: number): string {
    return minutes < 0 ? `−${formatDuration(-minutes)}` : formatDuration(minutes);
  }

  /** Returns a journey's estimated price, or `null` when none is known. */
  function priceText(parts: JourneyCostParts | null | undefined): string | null {
    const price = parts?.price ?? null;
    return price === null ? null : formatEuroRange(price);
  }

  /** Returns the rows of the details table: each part of the chosen journey and another. */
  function detailRows(other: JourneyCostParts) {
    const chosen = legPlan.chosen_journey.cost_parts;
    if (chosen === null || chosen === undefined) {
      return [];
    }
    const chosenCosts = partCosts(chosen);
    const otherCosts = partCosts(other);
    return COST_PART_NAMES.filter((part) => chosenCosts[part] !== 0 || otherCosts[part] !== 0).map(
      (part) => ({
        label: COST_PART_LABELS[part],
        chosen: formatCost(chosenCosts[part]),
        other: formatCost(otherCosts[part]),
      }),
    );
  }

  const alternativesWithParts = $derived(
    legPlan.alternative_journeys.filter(
      (alternative): alternative is Journey & { cost_parts: JourneyCostParts } =>
        alternative.cost_parts !== null && alternative.cost_parts !== undefined,
    ),
  );
</script>

<section class="explanation" aria-label="Why this journey">
  <p>{explainChoice(legPlan)}</p>

  {#if alternativesWithParts.length > 0}
    <ol class="compared">
      {#each alternativesWithParts as alternative, alternativeIndex (alternativeIndex)}
        {@const reason = explainAlternative(legPlan, alternative)}
        <li>
          <span class="journey-name">{describeJourney(legPlan, alternative)}</span>{reason === "" ? "" : `: ${reason}`}
          <details>
            <summary>Details</summary>
            <table>
              <thead>
                <tr><th></th><th>Shown journey</th><th>This one</th></tr>
              </thead>
              <tbody>
                {#each detailRows(alternative.cost_parts) as row (row.label)}
                  <tr><td>{row.label}</td><td>{row.chosen}</td><td>{row.other}</td></tr>
                {/each}
                {#if priceText(legPlan.chosen_journey.cost_parts) !== null || priceText(alternative.cost_parts) !== null}
                  <tr>
                    <td>Estimated price</td>
                    <td>{priceText(legPlan.chosen_journey.cost_parts) ?? "none"}</td>
                    <td>{priceText(alternative.cost_parts) ?? "none"}</td>
                  </tr>
                {/if}
                <tr class="total">
                  <td>Total</td>
                  <td>{formatCost(legPlan.chosen_journey.cost_parts?.total_cost ?? 0)}</td>
                  <td>{formatCost(alternative.cost_parts.total_cost)}</td>
                </tr>
              </tbody>
            </table>
          </details>
        </li>
      {/each}
    </ol>
  {/if}

  {#if nearbyDay !== null}
    <p>{explainNearbyDay(legPlan, nearbyDay)}</p>
  {/if}

  <p class="note">{explainUnits(legPlan)}</p>
  <p class="note">{explainSearch(legPlan)}</p>
</section>

<style>
  .explanation {
    margin-top: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius);
    background: var(--color-surface-muted);
  }

  .explanation p {
    margin: 0 0 var(--space-2);
  }

  .compared {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: 0 0 var(--space-2);
    padding-left: var(--space-4);
  }

  .journey-name {
    font-weight: 600;
  }

  table {
    margin-top: var(--space-1);
    border-collapse: collapse;
    font-variant-numeric: tabular-nums;
  }

  th,
  td {
    padding: 2px var(--space-2);
    text-align: right;
  }

  th:first-child,
  td:first-child {
    text-align: left;
  }

  .total td {
    border-top: 1px solid var(--color-border);
    font-weight: 600;
  }

  .note {
    color: var(--color-text-muted);
    font-size: 0.9em;
  }
</style>
