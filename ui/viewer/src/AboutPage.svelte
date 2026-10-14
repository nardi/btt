<script lang="ts">
  // What the viewer is, where its data comes from and what the data may be used for.
  import credits from "./generated/credits.json";

  let { onClose }: { onClose: () => void } = $props();

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
  <button type="button" class="back" onclick={onClose}>Back to the trip</button>
  <h1>About this page</h1>
  <p>
    This page shows a trip that someone planned in BigTrainTrip. It is a read-only copy. The trip
    travels inside the link, after the # sign, which browsers never send to a server. This page
    keeps no account, no cookies and no copy of the trip on any server.
  </p>
  <p>
    The only request that leaves the page is for the map. The tiles come from OpenFreeMap, which
    sees the area on screen but not the trip.
  </p>

  <h2>Timetable data</h2>
  <p>
    The trains and times come from the public timetable feeds below. The feeds are the ones in the
    timetable the planner used when this version of the viewer was released.
  </p>
  <ul class="feeds">
    {#each credits.feeds as feed (feed.name)}
      {@const address = safeAddress(feed.url)}
      <li>
        <span class="feed-name">{feedTitle(feed.name)}</span>
        <span class="feed-license">
          {feed.license === "unknown" ? "see the terms of the feed" : feed.license}
        </span>
        {#if address !== null}
          <a href={address} rel="noopener noreferrer" target="_blank">Source</a>
        {/if}
      </li>
    {/each}
  </ul>

  <h2>Map data</h2>
  <p>
    The map uses data from <a
      href="https://www.openstreetmap.org/copyright"
      rel="noopener noreferrer"
      target="_blank">OpenStreetMap contributors</a
    >, served as tiles by
    <a href="https://openfreemap.org" rel="noopener noreferrer" target="_blank">OpenFreeMap</a>.
  </p>

  <h2>Use of the data</h2>
  <p>
    The data is for personal and non-commercial use. Prices are estimates from the day the trip
    was planned, and they can differ from what a booking site charges.
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

  .feed-license {
    color: var(--color-text-muted);
  }
</style>
