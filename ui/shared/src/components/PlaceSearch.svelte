<script lang="ts">
  // Searches places by name and reports the one the user picks. The local search answers at
  // once. The online search adds its places below the local ones when it answers.
  import type { PlaceSearchResult } from "../bindings";
  import Spinner from "./Spinner.svelte";

  /**
   * Time without typing after which an online search starts, so each keystroke sends no
   * request.
   */
  const onlineSearchDelayMilliseconds = 300;

  let {
    searchPlaces,
    searchPlacesOnline,
    onPick,
    placeholder = "Add a place: city or station",
    focusOnMount = false,
  }: {
    /** Searches the places that the app knows without a network connection. */
    searchPlaces: (query: string) => Promise<PlaceSearchResult[]>;
    /** Searches the places that the local search does not find, with a network connection. */
    searchPlacesOnline?: (query: string) => Promise<PlaceSearchResult[]>;
    onPick: (result: PlaceSearchResult) => void;
    /** The hint of the empty search field. */
    placeholder?: string;
    /** Whether the field takes the keyboard focus when it appears. */
    focusOnMount?: boolean;
  } = $props();

  let field: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (focusOnMount) {
      field?.focus();
    }
  });

  let query = $state("");
  let localResults: PlaceSearchResult[] = $state([]);
  let onlineResults: PlaceSearchResult[] = $state([]);
  const results = $derived([...localResults, ...onlineResults]);
  let searchingOnline = $state(false);
  let pendingOnlineSearch: ReturnType<typeof setTimeout> | undefined;
  /** Counts queries, so an answer to an older query never replaces a newer one. */
  let latestSearchNumber = 0;

  /** Starts the local search at once and the online search once typing pauses. */
  function scheduleSearch(text: string) {
    query = text;
    clearTimeout(pendingOnlineSearch);
    const searchNumber = ++latestSearchNumber;
    onlineResults = [];
    searchingOnline = false;
    if (text.trim().length < 2) {
      localResults = [];
      return;
    }
    void searchPlaces(text).then((found) => {
      if (searchNumber === latestSearchNumber) {
        localResults = found;
      }
    });
    if (searchPlacesOnline === undefined) {
      return;
    }
    pendingOnlineSearch = setTimeout(async () => {
      searchingOnline = true;
      try {
        const found = await searchPlacesOnline(text);
        if (searchNumber === latestSearchNumber) {
          onlineResults = found;
        }
      } finally {
        if (searchNumber === latestSearchNumber) {
          searchingOnline = false;
        }
      }
    }, onlineSearchDelayMilliseconds);
  }

  function pick(result: PlaceSearchResult) {
    onPick(result);
    clearTimeout(pendingOnlineSearch);
    latestSearchNumber++;
    query = "";
    localResults = [];
    onlineResults = [];
    searchingOnline = false;
  }
</script>

<div class="place-search">
  <input
    bind:this={field}
    type="search"
    {placeholder}
    value={query}
    oninput={(event) => scheduleSearch(event.currentTarget.value)}
  />
  {#if searchingOnline}
    <span class="status">
      <Spinner label="Searching online" />
      Searching online…
    </span>
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
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  .results {
    list-style: none;
    margin: var(--space-1) 0 0;
    padding: 0;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
    overflow: hidden;
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
