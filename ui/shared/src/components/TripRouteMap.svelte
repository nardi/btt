<script lang="ts">
  // A small metro map of the route of a trip. The lines draw one after the other in the colors
  // of the legs, the stations pop up as the lines reach them, and a train then rides the whole
  // route, again and again. A reader who prefers less motion gets the finished map.
  import { prefersReducedMotion } from "../motion";
  import { routeMap, type RouteStop } from "../route-map";

  let {
    route,
  }: {
    /** The places of the trip in travel order. */
    route: readonly RouteStop[];
  } = $props();

  const id = $props.id();

  /** How fast a line draws, in pixels per second. */
  const drawSpeed = 130;
  /** How fast the train rides, in pixels per second. */
  const trainSpeed = 34;
  /** The pause before the train sets off, in seconds. */
  const trainDelay = 0.4;

  const map = $derived(routeMap(route, { width: 160, height: 76, padding: 14, grid: 6, bendRadius: 7 }));
  const animated = !prefersReducedMotion;

  /**
   * Returns when each line starts to draw and how long it takes, and when each station appears.
   * A station appears when the line that arrives at it is done, and the first one at the start.
   */
  const timeline = $derived.by(() => {
    const legs: { delay: number; duration: number }[] = [];
    let clock = 0;
    for (const leg of map?.legs ?? []) {
      const duration = Math.max(0.25, leg.length / drawSpeed);
      legs.push({ delay: clock, duration });
      clock += duration;
    }
    const stations = [0, ...legs.map((leg) => leg.delay + leg.duration)];
    return { legs, stations, total: clock };
  });
</script>

{#if map !== null}
  <svg
    class="route-map"
    class:animated
    width={map.width}
    height={map.height}
    viewBox={`0 0 ${map.width} ${map.height}`}
    aria-hidden="true"
  >
    <defs>
      <path id={`${id}-route`} d={map.path} fill="none" />
      {#each map.legs as leg, index (index)}
        {#if leg.night}
          <!-- A dashed line is drawn by revealing it with a solid line that draws in. -->
          <mask id={`${id}-reveal-${index}`}>
            <path
              class="reveal"
              d={leg.path}
              style:--length={leg.length}
              style:--delay={`${timeline.legs[index]?.delay ?? 0}s`}
              style:--duration={`${timeline.legs[index]?.duration ?? 0}s`}
            />
          </mask>
        {/if}
      {/each}
    </defs>

    {#each map.legs as leg, index (index)}
      {#if leg.night}
        <path class="line night" d={leg.path} stroke={leg.colour} mask={`url(#${id}-reveal-${index})`} />
      {:else}
        <path
          class="line draw"
          d={leg.path}
          stroke={leg.colour}
          style:--length={leg.length}
          style:--delay={`${timeline.legs[index]?.delay ?? 0}s`}
          style:--duration={`${timeline.legs[index]?.duration ?? 0}s`}
        />
      {/if}
    {/each}

    {#each map.stations as station, index (index)}
      <g class="station-pop" style:--delay={`${timeline.stations[index] ?? 0}s`}>
        {#if station.kind === "end"}
          <circle class="station" cx={station.x} cy={station.y} r="4.5" />
        {:else}
          <rect class="station" x={station.x - 4} y={station.y - 4} width="8" height="8" rx="2" />
        {/if}
      </g>
    {/each}

    {#if animated && map.legs.length > 0}
      <!-- The train waits at the last station before it goes back to the first. -->
      <rect class="train" x="-5" y="-3" width="10" height="6" rx="3" opacity="0">
        <set attributeName="opacity" to="1" begin={`${timeline.total + trainDelay}s`} fill="freeze" />
        <animateMotion
          dur={`${Math.max(2, map.length / trainSpeed) / 0.85}s`}
          begin={`${timeline.total + trainDelay}s`}
          repeatCount="indefinite"
          rotate="auto"
          calcMode="linear"
          keyPoints="0;1;1"
          keyTimes="0;0.85;1"
        >
          <mpath href={`#${id}-route`} />
        </animateMotion>
      </rect>
    {/if}
  </svg>
{/if}

<style>
  .route-map {
    display: block;
    flex: none;
    border-radius: 10px;
    background: var(--color-background);
  }

  .line {
    fill: none;
    stroke-width: 4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  /* A night leg is dashed in the night color, like on the itinerary. */
  .line.night {
    stroke-dasharray: 6 5;
    stroke-linecap: butt;
  }

  .reveal {
    fill: none;
    stroke: #ffffff;
    stroke-width: 8;
    stroke-linecap: butt;
    stroke-linejoin: round;
    stroke-dasharray: var(--length);
  }

  .station {
    fill: var(--color-surface);
    stroke: var(--color-text);
    stroke-width: 2;
  }

  .train {
    fill: var(--color-text);
  }

  .animated .draw {
    stroke-dasharray: var(--length);
    stroke-dashoffset: var(--length);
    animation: draw var(--duration) linear var(--delay) forwards;
  }

  .animated .reveal {
    stroke-dashoffset: var(--length);
    animation: draw var(--duration) linear var(--delay) forwards;
  }

  .animated .station-pop {
    opacity: 0;
    transform-box: fill-box;
    transform-origin: center;
    animation: pop 250ms ease-out var(--delay) forwards;
  }

  @keyframes draw {
    to {
      stroke-dashoffset: 0;
    }
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.3);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
