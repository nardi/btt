<script lang="ts">
  // Searches places by name and reports the one the user picks.
  import type { PlaceSearchResult } from "../bindings";

  /** Time without typing after which a search starts, so each keystroke sends no request. */
  const searchDelayMilliseconds = 300;

  let {
    searchPlaces,
    onPick,
  }: {
    searchPlaces: (query: string) => Promise<PlaceSearchResult[]>;
    onPick: (result: PlaceSearchResult) => void;
  } = $props();

  let query = $state("");
  let results: PlaceSearchResult[] = $state([]);
  let searching = $state(false);
  let pendingSearch: ReturnType<typeof setTimeout> | undefined;
  /** Counts searches, so an answer to an older query never replaces a newer one. */
  let latestSearchNumber = 0;

  /** Starts a search once typing pauses. */
  function scheduleSearch(text: string) {
    query = text;
    clearTimeout(pendingSearch);
    if (text.trim().length < 2) {
      results = [];
      return;
    }
    pendingSearch = setTimeout(async () => {
      const searchNumber = ++latestSearchNumber;
      searching = true;
      try {
        const found = await searchPlaces(text);
        if (searchNumber === latestSearchNumber) {
          results = found;
        }
      } finally {
        if (searchNumber === latestSearchNumber) {
          searching = false;
        }
      }
    }, searchDelayMilliseconds);
  }

  function pick(result: PlaceSearchResult) {
    onPick(result);
    query = "";
    results = [];
  }
</script>

<div class="place-search">
  <input
    type="search"
    placeholder="Add a place: city or station"
    value={query}
    oninput={(event) => scheduleSearch(event.currentTarget.value)}
  />
  {#if searching}
    <span class="status">Searching…</span>
  {/if}
  {#if results.length > 0}
    <ul class="results">
      {#each results as result, resultIndex (resultIndex)}
        <li>
          <button type="button" onclick={() => pick(result)}>
            <span class="name">{result.name}</span>
            <span class="context">{result.context}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .place-search {
    position: relative;
  }

  input {
    width: 100%;
    box-sizing: border-box;
    padding: var(--space-2) var(--space-3);
  }

  .status {
    position: absolute;
    right: var(--space-3);
    top: var(--space-2);
    color: var(--color-text-muted);
  }

  .results {
    list-style: none;
    margin: var(--space-1) 0 0;
    padding: 0;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  .results button {
    display: flex;
    gap: var(--space-2);
    width: 100%;
    border: none;
    border-radius: 0;
    text-align: left;
    padding: var(--space-2) var(--space-3);
  }

  .results button:hover {
    background: var(--color-surface-muted);
  }

  .context {
    color: var(--color-text-muted);
  }
</style>
