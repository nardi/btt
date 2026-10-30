<script lang="ts">
  // What the app is, which requests it makes and where its data comes from.
  import { commands, type DataLocationStatus, type FeedCredit } from "@btt/shared/bindings";
  import Icon from "@btt/shared/components/Icon.svelte";
  import { onMount } from "svelte";
  import { commandErrorMessage } from "./tauri-trip-source.svelte";

  let { onClose }: { onClose: () => void } = $props();

  /** The feeds of the installed timetable, or `null` until the backend answered. */
  let feeds: FeedCredit[] | null = $state(null);

  /** Whether the data sits next to the app, or `null` until the backend answered. */
  let locationStatus: DataLocationStatus | null = $state(null);

  /** The message of the last failed change of the data location. */
  let locationMessage: string | null = $state(null);

  onMount(async () => {
    feeds = await commands.timetableFeeds();
    await loadLocationStatus();
  });

  /** Loads whether the data sits next to the app and whether a restart is pending. */
  async function loadLocationStatus() {
    const result = await commands.dataLocationStatus();
    if (result.status === "ok") {
      locationStatus = result.data;
    } else {
      locationMessage = commandErrorMessage(result.error);
    }
  }

  /** Saves portable mode. The data moves at the next start, so the page offers a restart. */
  async function setPortable(enabled: boolean) {
    const result = await commands.setPortableMode(enabled);
    if (result.status === "error") {
      locationMessage = commandErrorMessage(result.error);
    } else {
      locationMessage = null;
    }
    await loadLocationStatus();
  }

  /** Returns the address of a feed when it is a web address the page may link to. */
  function safeAddress(address: string): string | null {
    try {
      return new URL(address).protocol === "https:" ? address : null;
    } catch {
      return null;
    }
  }

  /** Returns the feed name without its country prefix, such as `DELFI` for `de_DELFI`. */
  function feedTitle(name: string): string {
    return name.replace(/^[a-z]{2}_/, "").replaceAll("-", " ").replaceAll("_", " ");
  }
</script>

<article class="about">
  <button type="button" class="with-icon back" onclick={onClose}>
    <Icon name="back" />
    Back
  </button>
  <h1>About BigTrainTrip</h1>
  <p>
    BigTrainTrip plans long train trips across Europe and estimates what their tickets cost. Your
    trips are stored on this computer and the app has no account.
  </p>
  <p>
    The app makes three kinds of requests. It downloads its timetable file from the releases of
    the project on GitHub. It loads map tiles from OpenFreeMap, which sees the area on screen but
    not the trip. When you price a trip, it asks some booking sites for the live price of a
    ticket, and those sites see the stations and the dates of that ticket.
  </p>

  <h2>Timetable data</h2>
  <p>The trains and times come from the public timetable feeds below.</p>
  {#if feeds === null}
    <p class="muted">Loading the list of feeds.</p>
  {:else if feeds.length === 0}
    <p class="muted">The app has no timetable installed.</p>
  {:else}
    <ul class="feeds">
      {#each feeds as feed (feed.name)}
        {@const address = safeAddress(feed.url)}
        <li>
          <span class="feed-name">{feedTitle(feed.name)}</span>
          <span class="muted">
            {feed.license === "unknown" ? "see the terms of the feed" : feed.license}
          </span>
          {#if address !== null}
            <a href={address} rel="noopener noreferrer" target="_blank">Source</a>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <h2>Train layouts</h2>
  <p>
    The button beside a train opens its page on
    <a href="https://www.vagonweb.cz" rel="noopener noreferrer" target="_blank">vagonWEB</a>,
    which shows the order of the coaches. The app only links to these pages and never copies
    them.
  </p>

  <h2>Map data</h2>
  <p>
    The map uses data from <a
      href="https://www.openstreetmap.org/copyright"
      rel="noopener noreferrer"
      target="_blank">OpenStreetMap contributors</a
    >, served as tiles by
    <a href="https://openfreemap.org" rel="noopener noreferrer" target="_blank">OpenFreeMap</a>.
  </p>

  <h2>Data location</h2>
  {#if locationStatus === null}
    <p class="muted">Loading the data location.</p>
  {:else}
    <label class="option">
      <input
        type="checkbox"
        checked={locationStatus.portable}
        onchange={(event) => void setPortable(event.currentTarget.checked)}
      />
      Portable mode
    </label>
    <p>
      In portable mode, the trips, travellers and timetable go into a folder named
      <code>data</code> beside the app. Without it, they stay in your user profile.
    </p>
    {#if locationStatus.restart_pending}
      <p>The data moves when the app restarts.</p>
      <button type="button" onclick={() => void commands.restartApp()}>Restart now</button>
    {/if}
  {/if}
  {#if locationMessage !== null}
    <p class="message" role="alert">{locationMessage}</p>
  {/if}

  <h2>Use of the data</h2>
  <p>
    The data is for personal and non-commercial use. Prices are estimates, and they can differ
    from what a booking site charges.
  </p>
</article>

<style>
  .about {
    max-width: 640px;
    margin: 0 auto;
    padding: var(--space-4) 0 var(--space-6);
  }

  .back {
    margin-bottom: var(--space-4);
  }

  h1 {
    font-size: var(--text-title);
    margin: 0 0 var(--space-3);
  }

  h2 {
    font-size: var(--text-section);
    margin: var(--space-6) 0 var(--space-2);
  }

  .feeds {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-small);
  }

  .feeds li {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: baseline;
  }

  .feed-name {
    font-weight: 600;
  }

  .muted,
  .message {
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }

  .option {
    display: flex;
    align-items: flex-start;
    gap: var(--space-2);
  }
</style>
