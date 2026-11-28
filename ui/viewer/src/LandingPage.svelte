<script lang="ts">
  // The page at the root of the site when the address holds no trip. It says what BigTrainTrip
  // is, links to the web app and the desktop releases, and offers the ways to open a shared trip.

  let {
    onScan,
    onOpenFile,
  }: {
    /** Opens the camera to scan the QR code of a trip. */
    onScan: () => void;
    /** Opens the file picker for a trip file or a picture of a QR code. */
    onOpenFile: () => void;
  } = $props();

  /** Address of the web app, relative to the root of the site. */
  const webAppAddress = "app/";

  /** Address of the newest release, which has the installers and the portable archives. */
  const releasesAddress = "https://github.com/nardi/btt/releases/latest";

  /** Address of the source code. */
  const repositoryAddress = "https://github.com/nardi/btt";

  /** The main points of the app, each with a short title and a few sentences. */
  const features = [
    {
      title: "Multi-day trips",
      text:
        "Plan a trip over as many days as you need, with up to 255 stops. You can add a " +
        "destination or change the order at any time, and the trip is planned again in seconds.",
    },
    {
      title: "Offline planning",
      text:
        "The planner runs on your device. When the timetable is downloaded, you can plan without " +
        "an internet connection. Only the map, the place search and live prices need one.",
    },
    {
      title: "Ticket prices",
      text:
        "You see an estimate for every ticket, and the desktop app also looks up live prices on " +
        "booking sites. The app tells you when an Interrail pass is cheaper than single tickets.",
    },
    {
      title: "Travel preferences",
      text:
        "Choose when you depart and arrive, how long you stay, how much you mind changing trains " +
        "and how long a transfer takes. For a night train, set your bedtime and the berth you " +
        "want. Options for bikes and accessible trains are coming soon.",
    },
  ];

  /** The list of features, which is a carousel on a phone. */
  let featureList: HTMLUListElement | undefined = $state();

  /** The feature that the carousel shows, for the dots below it. */
  let shownFeatureIndex = $state(0);

  /** Finds the feature that is nearest to the start of the carousel after a scroll. */
  function updateShownFeature() {
    const list = featureList;
    const firstItem = list?.firstElementChild;
    if (list === undefined || !(firstItem instanceof HTMLElement)) {
      return;
    }
    const step = firstItem.offsetWidth + parseFloat(getComputedStyle(list).columnGap || "0");
    shownFeatureIndex = Math.min(features.length - 1, Math.max(0, Math.round(list.scrollLeft / step)));
  }

  /** Scrolls the carousel to a feature. */
  function showFeature(index: number) {
    const item = featureList?.children[index];
    if (item instanceof HTMLElement && featureList !== undefined) {
      // The list is positioned, so the offset of an item counts from the padding edge of the list.
      const padding = parseFloat(getComputedStyle(featureList).paddingLeft);
      featureList.scrollTo({ left: item.offsetLeft - padding, behavior: "smooth" });
    }
  }
</script>

<article class="landing">
  <section class="hero">
    <!-- Four legs in the colours of the itinerary, as a picture of a trip with several stops. -->
    <svg class="route" viewBox="0 0 320 40" aria-hidden="true">
      <path class="leg leg-1" d="M8 30 H72" />
      <path class="leg leg-2" d="M88 30 Q118 30 126 18 Q134 8 152 8" />
      <path class="leg leg-3" d="M168 8 H232" />
      <path class="leg leg-4" d="M248 8 Q270 8 280 22 Q288 30 312 30" />
      <circle class="stop end" cx="8" cy="30" r="5" />
      <rect class="stop" x="74" y="24" width="12" height="12" rx="2" />
      <rect class="stop" x="154" y="2" width="12" height="12" rx="2" />
      <rect class="stop" x="234" y="2" width="12" height="12" rx="2" />
      <circle class="stop end" cx="312" cy="30" r="5" />
    </svg>
    <h1>Plan and price long train trips across Europe</h1>
    <p class="lead">
      Tell BigTrainTrip which places you want to visit and how long you want to stay in each. It
      picks a journey for every leg, prefers night trains where they save you a hotel night, and
      estimates what the tickets cost.
    </p>
    <div class="calls">
      <a class="link-button primary" href={webAppAddress}>Open the web app</a>
      <a class="link-button" href={releasesAddress} rel="noopener noreferrer">Download for desktop</a>
    </div>
  </section>

  <section aria-label="Features">
    <ul class="features" bind:this={featureList} onscroll={updateShownFeature}>
      {#each features as feature (feature.title)}
        <li>
          <h2>{feature.title}</h2>
          <p>{feature.text}</p>
        </li>
      {/each}
    </ul>
    <div class="dots">
      {#each features as feature, index (feature.title)}
        <button
          type="button"
          class="dot"
          class:shown={index === shownFeatureIndex}
          aria-label={`Show ${feature.title}`}
          aria-current={index === shownFeatureIndex}
          onclick={() => showFeature(index)}
        ></button>
      {/each}
    </div>
  </section>

  <section>
    <h2 class="section-title">Web app and desktop app</h2>
    <p>You can use BigTrainTrip in your browser, or you can install it on your computer.</p>
    <div class="versions">
      <div class="card">
        <h3>Web app</h3>
        <p>
          If you want to start right away, open the web app. It runs in the browser on a computer,
          tablet or phone, and you do not need to install anything. On your first visit, it
          downloads the timetable, which is about 12 MB.
        </p>
        <p class="muted">
          Booking sites do not accept requests from a web page, so the web app shows estimated
          prices only. Your trips are stored in this browser.
        </p>
        <a class="link-button primary" href={webAppAddress}>Open the web app</a>
      </div>
      <div class="card">
        <h3>Desktop app</h3>
        <p>
          If you want live prices, download the desktop app for Windows, macOS or Linux. You can
          install it, or you can run the portable version without installation. The desktop app
          also plans long legs faster.
        </p>
        <p class="muted">
          The installers are not signed, so Windows and macOS ask you to confirm that you want to
          open them.
        </p>
        <a class="link-button" href={releasesAddress} rel="noopener noreferrer">Download</a>
      </div>
    </div>
    <p class="muted">
      If you want to move a trip from one app to the other, save the trip as a file in the share
      dialog. Then import the file in the trip list of the other app.
    </p>
  </section>

  <section>
    <h2 class="section-title">Shared trips</h2>
    <p>
      If someone sent you a link to a trip, you can open the link to view the trip they planned. The
      trip then shows on this page. If you have the QR code of a trip, you can scan it with your
      camera or open a picture of it. You can also open a trip file.
    </p>
    <p class="muted">
      The trip stays on your device. This page only loads the map tiles from a server.
    </p>
    <div class="calls">
      <button type="button" onclick={onScan}>Scan a QR code</button>
      <button type="button" onclick={onOpenFile}>Open a file</button>
    </div>
  </section>

  <footer class="muted">
    BigTrainTrip is open source. You can find the code on
    <a href={repositoryAddress} rel="noopener noreferrer">GitHub</a>.
  </footer>
</article>

<style>
  /* The page takes the full width of its column. Without a width, the auto margins size it to
     its content, and the row of features on a phone is wider than the screen. */
  .landing {
    width: 100%;
    min-width: 0;
    max-width: 880px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: calc(2 * var(--space-6));
    padding: var(--space-6) 0;
  }

  .hero {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-width: 640px;
  }

  .route {
    width: 200px;
    height: auto;
  }

  .leg {
    fill: none;
    stroke-width: 5;
    stroke-linecap: round;
  }

  .leg-1 {
    stroke: var(--leg-1);
  }

  .leg-2 {
    stroke: var(--leg-2);
  }

  .leg-3 {
    stroke: var(--leg-3);
  }

  .leg-4 {
    stroke: var(--leg-4);
  }

  .stop {
    fill: var(--color-text);
  }

  .stop.end {
    fill: var(--color-surface);
    stroke: var(--color-text);
    stroke-width: 3;
  }

  h1 {
    margin: 0;
    font-size: var(--text-figure);
    line-height: 1.15;
  }

  .lead {
    margin: 0;
    font-size: var(--text-lead);
    color: var(--color-text-muted);
  }

  .calls {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  /* A link that looks like a button of the app, for links that lead to another page. */
  .link-button {
    display: inline-block;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-text);
    border-radius: var(--radius-pill);
    padding: 7px 16px;
    font-size: var(--text-button);
    font-weight: 600;
    text-decoration: none;
    transition:
      background-color 130ms ease,
      border-color 130ms ease;
  }

  .link-button:hover {
    background: var(--color-surface-muted);
    border-color: var(--color-text-muted);
  }

  .link-button.primary {
    background: var(--color-accent);
    border-color: var(--color-accent);
    color: var(--color-accent-contrast);
  }

  .link-button.primary:hover {
    background: color-mix(in srgb, var(--color-accent) 84%, var(--color-text));
    border-color: color-mix(in srgb, var(--color-accent) 84%, var(--color-text));
  }

  .link-button:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }

  .features {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: var(--space-4);
  }

  .features h2 {
    margin: 0 0 var(--space-1);
    font-size: var(--text-heading);
  }

  .features p {
    margin: 0;
    color: var(--color-text-muted);
  }

  /* The dots of the carousel only show on a phone. */
  .dots {
    display: none;
    justify-content: center;
    gap: var(--space-1);
    margin-top: var(--space-2);
  }

  .dot {
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    background: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  /* The visible dot is smaller than the button, so the button is easy to touch. */
  .dot::before {
    content: "";
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--color-border);
    transition: background-color 130ms ease;
  }

  .dot:hover:not(:disabled) {
    background: none;
  }

  .dot.shown::before {
    background: var(--color-accent);
  }

  section > p {
    max-width: 640px;
  }

  .section-title {
    margin: 0 0 var(--space-3);
    font-size: var(--text-section);
  }

  .versions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: var(--space-3);
    margin-bottom: var(--space-3);
  }

  .card {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--color-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .card h3 {
    margin: 0;
    font-size: var(--text-heading);
  }

  .card p {
    margin: 0;
  }

  /* The button of each card sits at the bottom, so the two buttons line up. */
  .card .link-button {
    margin-top: auto;
  }

  .muted {
    color: var(--color-text-muted);
    font-size: var(--text-small);
  }

  section p {
    margin-top: 0;
  }

  footer a {
    color: inherit;
  }

  @media (max-width: 719px) {
    .landing {
      gap: var(--space-6);
      padding: var(--space-3) 0;
    }

    h1 {
      font-size: var(--text-title);
    }

    /* On a phone, the features are a row of cards that scrolls sideways. The row reaches to
       the edges of the screen, and each card leaves a part of the next one visible. */
    .features {
      position: relative;
      display: flex;
      overflow-x: auto;
      scroll-snap-type: x mandatory;
      scrollbar-width: none;
      gap: var(--space-3);
      margin: 0 calc(-1 * var(--space-4));
      padding: 0 var(--space-4);
      scroll-padding: 0 var(--space-4);
    }

    .features::-webkit-scrollbar {
      display: none;
    }

    .features li {
      flex: 0 0 82%;
      box-sizing: border-box;
      scroll-snap-align: start;
      padding: var(--space-4);
      background: var(--color-surface);
      border: 1px solid var(--color-border);
      border-radius: var(--radius);
    }

    .dots {
      display: flex;
    }
  }
</style>
