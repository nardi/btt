<script lang="ts">
  // Map of the trip: places as markers, station search radii as circles, planned rides as lines
  // and the ways between places and stations as dotted lines. In an editable source, double
  // clicks add places and drags move them or change their radius.
  import "maplibre-gl/dist/maplibre-gl.css";

  import type { GeoCoordinates, TripEdit, TripPlace, TripPlaceId } from "../bindings";
  import type {
    ExpressionSpecification,
    Map as MapLibreMap,
    MapLayerMouseEvent,
    MapMouseEvent,
  } from "maplibre-gl";
  import {
    CircleLayer,
    FillLayer,
    GeoJSONSource,
    LineLayer,
    MapLibre,
    Marker,
    Popup,
  } from "svelte-maplibre-gl";

  import { destinationPoint, haversineMeters } from "../geo";
  import { itineraryRows, tripPlaces } from "../itinerary";
  import {
    type PlaceDragPreview,
    type PlaceFocusRequest,
    placeBounds,
    radiusCircles,
    rideLines,
    stationWayLines,
    intermediateStopPoints,
    transferPoints,
  } from "../map-features";
  import {
    legPalette,
    mapAccent,
    mapInk,
    mapMuted,
    mapStyle,
    mapSurface,
    nightColour,
    theme,
  } from "../theme.svelte";
  import type { TripSource } from "../trip-source";

  /** The derived base style of the current theme, which keeps the OpenFreeMap attribution. */
  const baseStyle = $derived(mapStyle(theme.current));
  /** Limits of a station search radius, matching `StationSearchRadius` in Rust. */
  const minimumRadiusMeters = 100;
  const maximumRadiusMeters = 50_000;
  /** Radius handles snap to this step while dragging. */
  const radiusStepMeters = 100;

  /** Zoom level the map moves to at least when it shows a single place. */
  const placeFocusZoom = 10;
  /** Below this zoom level the map shows too much to judge the stations around a place. */
  const overviewZoomLimit = 8;
  /** Longest time a move to a place takes. Far moves would otherwise fly for several seconds. */
  const longestMoveMilliseconds = 1500;

  let {
    source,
    selectedPlaceId,
    focusRequest,
    onSelect,
    onViewChange,
  }: {
    source: TripSource;
    selectedPlaceId: TripPlaceId | null;
    /** Asks the map to move to a place, for example after a double click in the list. */
    focusRequest: PlaceFocusRequest | null;
    onSelect: (placeId: TripPlaceId | null) => void;
    /** Receives the centre and zoom of the map after it moves. */
    onViewChange?: (view: { center: GeoCoordinates; zoom: number }) => void;
  } = $props();

  /** The length of the equator, which sets the scale of the map at each zoom level. */
  const earthCircumferenceMeters = 40_075_017;
  /** Half the size of a place marker. */
  const markerRadiusPixels = 11;
  /** How far beyond the marker's edge the handle grows to its full size. */
  const handleGrowthPixels = 24;
  /** Below this size the handle is hidden enough that it takes no drags. */
  const handleActiveScale = 0.35;

  let map: MapLibreMap | undefined = $state.raw();
  /** The zoom level of the map, which sets the size of the radius handles. */
  let zoom = $state(4);
  let dragPreview: PlaceDragPreview | null = $state(null);
  /** Trip whose places the map last zoomed to, so edits never move the view. */
  let fittedTripId: string | null = null;
  /** Places the map has seen for the current trip, so it notices added ones. */
  let knownPlaceIds = new Set<TripPlaceId>();
  /** Last focus request the map moved for, so trip edits do not repeat the move. */
  let handledFocusRequestNumber = 0;

  const trip = $derived(source.viewState.trip);
  const places = $derived(tripPlaces(trip.itinerary));
  /** The places that are stops in between, which the map draws as squares like the itinerary. */
  const viaPlaceIds = $derived(
    new Set(itineraryRows(trip).places.filter((row) => row.role === "via").map((row) => row.place.id)),
  );
  const editable = $derived(source.kind === "editable" ? source : null);
  const circles = $derived(radiusCircles(trip, selectedPlaceId, dragPreview));
  const rides = $derived(rideLines(source.viewState));
  const stationWays = $derived(stationWayLines(source.viewState));
  const intermediateStops = $derived(intermediateStopPoints(source.viewState));
  const transfers = $derived(transferPoints(source.viewState));

  /** The colour of a day ride, by the leg's position in the cycling palette. */
  const dayRideColour = $derived(
    [
      "match",
      ["get", "legPosition"],
      1,
      legPalette[theme.current][0],
      2,
      legPalette[theme.current][1],
      3,
      legPalette[theme.current][2],
      4,
      legPalette[theme.current][3],
      legPalette[theme.current][0],
    ] as ExpressionSpecification,
  );

  /** The feature under the pointer and where its tooltip goes, or `null` when there is none. */
  let tooltip: { lnglat: [number, number]; lines: string[] } | null = $state(null);

  /** Shows the tooltip of the feature under the pointer, which carries its text as a property. */
  function showTooltip(event: MapLayerMouseEvent) {
    const text = event.features?.[0]?.properties?.["tooltip"];
    if (typeof text !== "string") {
      return;
    }
    tooltip = { lnglat: [event.lngLat.lng, event.lngLat.lat], lines: text.split("\n") };
    if (map !== undefined) {
      map.getCanvas().style.cursor = "pointer";
    }
  }

  /** Tells the parent where the map looks after it moved. */
  function reportView() {
    if (map !== undefined) {
      const center = map.getCenter();
      onViewChange?.({ center: { latitude: center.lat, longitude: center.lng }, zoom: map.getZoom() });
    }
  }

  function hideTooltip() {
    tooltip = null;
    if (map !== undefined) {
      map.getCanvas().style.cursor = "";
    }
  }

  // A remount of the map, such as when the theme changes, needs to fit the trip again.
  $effect(() => {
    void map;
    fittedTripId = null;
    knownPlaceIds = new Set();
  });

  // Zooms to all places when a trip opens.
  $effect(() => {
    const bounds = placeBounds(trip);
    if (map === undefined || fittedTripId === trip.id) {
      return;
    }
    fittedTripId = trip.id;
    knownPlaceIds = new Set(places.map((place) => place.id));
    if (bounds !== null) {
      map.fitBounds(bounds, { padding: 80, maxZoom: placeFocusZoom, duration: 0 });
    }
  });

  // Moves to an added place when the view does not show it well: when it lies outside the view,
  // as after a search, or when the map is zoomed out too far to show the stations around it.
  $effect(() => {
    if (map === undefined || fittedTripId !== trip.id) {
      return;
    }
    const addedPlaces = places.filter((place) => !knownPlaceIds.has(place.id));
    knownPlaceIds = new Set(places.map((place) => place.id));
    const lastAdded = addedPlaces.at(-1);
    if (
      lastAdded !== undefined &&
      (!isInView(map, lastAdded.coordinates) || map.getZoom() < overviewZoomLimit)
    ) {
      moveTo(map, lastAdded.coordinates);
    }
  });

  // Moves to the place of a new focus request.
  $effect(() => {
    if (map === undefined || focusRequest === null) {
      return;
    }
    if (focusRequest.requestNumber === handledFocusRequestNumber) {
      return;
    }
    const place = places.find((candidate) => candidate.id === focusRequest.placeId);
    if (place !== undefined) {
      handledFocusRequestNumber = focusRequest.requestNumber;
      moveTo(map, place.coordinates);
    }
  });

  function isInView(targetMap: MapLibreMap, coordinates: GeoCoordinates): boolean {
    return targetMap.getBounds().contains([coordinates.longitude, coordinates.latitude]);
  }

  /** Flies to a place and zooms in far enough to see the stations around it. */
  function moveTo(targetMap: MapLibreMap, coordinates: GeoCoordinates) {
    targetMap.flyTo({
      center: [coordinates.longitude, coordinates.latitude],
      zoom: Math.max(targetMap.getZoom(), placeFocusZoom),
      maxDuration: longestMoveMilliseconds,
    });
  }

  function edit(tripEdit: TripEdit) {
    void editable?.applyEdit(tripEdit);
  }

  function lngLatToCoordinates(lngLat: { lng: number; lat: number }): GeoCoordinates {
    return { latitude: lngLat.lat, longitude: lngLat.lng };
  }

  /** Returns whether a map event started on a marker. Marker events bubble to the map. */
  function startedOnMarker(event: MapMouseEvent): boolean {
    const target = event.originalEvent.target;
    return target instanceof Element && target.closest(".maplibregl-marker") !== null;
  }

  /** Clears the selection on a single click on the map itself. */
  function handleMapClick(event: MapMouseEvent) {
    if (!startedOnMarker(event)) {
      onSelect(null);
    }
  }

  /**
   * Adds a place on a double click on the map itself. A single click adds nothing, because it
   * happens too easily while panning.
   */
  function handleMapDoubleClick(event: MapMouseEvent) {
    if (editable !== null && !startedOnMarker(event)) {
      void editable.addPlaceAt(lngLatToCoordinates(event.lngLat));
    }
  }

  /**
   * Returns how large the radius handle of a place draws, from 0 to 1. Zoomed out, the radius is
   * only a few pixels and the handle would sit on the place marker, so it shrinks and hides
   * behind the marker.
   */
  function handleScale(place: TripPlace): number {
    const preview = dragPreview?.placeId === place.id ? dragPreview : null;
    const radiusMeters = preview?.radiusMeters ?? place.station_search_radius;
    const latitude = (place.coordinates.latitude * Math.PI) / 180;
    // MapLibre draws the world 512 pixels wide at zoom 0.
    const metersPerPixel = (earthCircumferenceMeters * Math.cos(latitude)) / (512 * 2 ** zoom);
    const gapPixels = radiusMeters / metersPerPixel - markerRadiusPixels;
    return Math.min(1, Math.max(0, gapPixels / handleGrowthPixels));
  }

  function radiusHandlePosition(place: TripPlace): [number, number] {
    const preview = dragPreview?.placeId === place.id ? dragPreview : null;
    const center = preview?.coordinates ?? place.coordinates;
    const radiusMeters = preview?.radiusMeters ?? place.station_search_radius;
    const handle = destinationPoint(center, 90, radiusMeters);
    return [handle.longitude, handle.latitude];
  }

  /** Clamps a dragged radius to the allowed range and snaps it to the step. */
  function snappedRadius(meters: number): number {
    const snapped = Math.round(meters / radiusStepMeters) * radiusStepMeters;
    return Math.min(maximumRadiusMeters, Math.max(minimumRadiusMeters, snapped));
  }

  function previewMove(place: TripPlace, lngLat: { lng: number; lat: number }) {
    dragPreview = {
      placeId: place.id,
      coordinates: lngLatToCoordinates(lngLat),
      radiusMeters: place.station_search_radius,
    };
  }

  function finishMove(place: TripPlace, lngLat: { lng: number; lat: number }) {
    dragPreview = null;
    edit({ MovePlace: { place_id: place.id, coordinates: lngLatToCoordinates(lngLat) } });
  }

  function previewRadius(place: TripPlace, lngLat: { lng: number; lat: number }) {
    const radiusMeters = snappedRadius(haversineMeters(place.coordinates, lngLatToCoordinates(lngLat)));
    dragPreview = { placeId: place.id, coordinates: place.coordinates, radiusMeters };
  }

  function finishRadius(place: TripPlace) {
    const radius = dragPreview?.placeId === place.id ? dragPreview.radiusMeters : null;
    dragPreview = null;
    if (radius !== null && radius !== place.station_search_radius) {
      edit({ SetStationSearchRadius: { place_id: place.id, radius } });
    }
  }
</script>

<MapLibre
  bind:map
  class="trip-map"
  style={baseStyle}
  autoloadGlobalCss={false}
  center={[10, 50]}
  bind:zoom
  doubleClickZoom={editable === null}
  onclick={handleMapClick}
  ondblclick={handleMapDoubleClick}
  onmoveend={reportView}
>
  <!-- A read-only map draws no station search radii, because a shared trip does not keep them. -->
  {#if editable !== null}
    <GeoJSONSource id="station-search-radii" data={circles}>
      <FillLayer
        paint={{
          "fill-color": mapAccent[theme.current],
          "fill-opacity": ["case", ["get", "selected"], 0.16, 0.07],
        }}
      />
      <LineLayer
        paint={{ "line-color": mapAccent[theme.current], "line-width": 1.5, "line-opacity": 0.6 }}
      />
    </GeoJSONSource>
  {/if}

  <GeoJSONSource id="station-ways" data={stationWays}>
    <LineLayer
      layout={{ "line-cap": "round" }}
      paint={{
        "line-color": mapMuted[theme.current],
        "line-width": 2,
        "line-dasharray": [0.5, 2],
      }}
    />
  </GeoJSONSource>

  <GeoJSONSource id="planned-rides" data={rides}>
    <!-- Day rides take the cycling palette by leg position. -->
    <LineLayer
      filter={["==", ["get", "night"], false]}
      layout={{ "line-cap": "round" }}
      paint={{ "line-color": dayRideColour, "line-width": 4 }}
    />
    <!-- A night leg stays dashed and purple. -->
    <LineLayer
      filter={["==", ["get", "night"], true]}
      layout={{ "line-cap": "round" }}
      paint={{
        "line-color": nightColour[theme.current],
        "line-width": 4,
        "line-dasharray": [2, 1.5],
      }}
    />
    <!-- A wide invisible line catches the pointer, because a 4 pixel line is hard to hit. -->
    <LineLayer
      paint={{ "line-color": "#000000", "line-opacity": 0, "line-width": 14 }}
      onmousemove={showTooltip}
      onmouseleave={hideTooltip}
    />
  </GeoJSONSource>

  <!-- Small dots for stops the traveller stays on the train through. They take no pointer
       events, so hovering one still shows the tooltip of the line below. -->
  <GeoJSONSource id="intermediate-stops" data={intermediateStops}>
    <CircleLayer
      paint={{
        "circle-radius": 2.5,
        "circle-color": [
          "case",
          ["get", "night"],
          nightColour[theme.current],
          dayRideColour,
        ] as ExpressionSpecification,
      }}
    />
  </GeoJSONSource>

  <GeoJSONSource id="transfer-points" data={transfers}>
    <CircleLayer
      paint={{
        "circle-radius": ["case", ["==", ["get", "kind"], "transfer"], 6, 4],
        "circle-color": mapSurface[theme.current],
        "circle-stroke-color": mapInk[theme.current],
        "circle-stroke-width": ["case", ["==", ["get", "kind"], "transfer"], 3, 2],
      }}
      onmousemove={showTooltip}
      onmouseleave={hideTooltip}
    />
  </GeoJSONSource>

  {#if tooltip !== null}
    <Popup
      lnglat={tooltip.lnglat}
      open
      closeButton={false}
      closeOnClick={false}
      offset={10}
      maxWidth="280px"
    >
      <div class="map-tooltip">
        {#each tooltip.lines as line, lineIndex (lineIndex)}
          <div class:title={lineIndex === 0}>{line}</div>
        {/each}
      </div>
    </Popup>
  {/if}

  {#each places as place, index (place.id)}
    <!-- The handle comes first, so the place marker covers it where the two meet. -->
    {#if editable !== null}
      {@const scale = handleScale(place)}
      <Marker
        lnglat={radiusHandlePosition(place)}
        draggable={scale >= handleActiveScale}
        ondrag={(event) => previewRadius(place, event.target.getLngLat())}
        ondragend={() => finishRadius(place)}
      >
        {#snippet content()}
          <div
            class="radius-handle"
            class:inactive={scale < handleActiveScale}
            style:--handle-scale={scale}
            title="Drag to change the station search radius"
          ></div>
        {/snippet}
      </Marker>
    {/if}
    <Marker
      lnglat={[place.coordinates.longitude, place.coordinates.latitude]}
      draggable={editable !== null}
      ondrag={(event) => previewMove(place, event.target.getLngLat())}
      ondragend={(event) => finishMove(place, event.target.getLngLat())}
    >
      {#snippet content()}
        <button
          type="button"
          class="place-marker"
          class:editable={editable !== null}
          class:selected={place.id === selectedPlaceId}
          class:via={viaPlaceIds.has(place.id)}
          title={editable === null ? place.name : `${place.name}. Drag to move the place.`}
          onclick={() => onSelect(place.id)}
        >
          {index + 1}
        </button>
      {/snippet}
    </Marker>
  {/each}
</MapLibre>

<style>
  :global(.trip-map) {
    width: 100%;
    height: 100%;
  }

  /* The places are small numbered ink circles, with a thin keyline in the background colour. */
  .place-marker {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--color-text);
    color: var(--color-background);
    font-size: 11px;
    font-weight: 600;
    box-shadow: 0 0 0 1.5px var(--color-background);
  }

  /* A place that can move shows the grab cursor, so a drag is the expected action. */
  .place-marker.editable {
    cursor: grab;
  }

  .place-marker.editable:active {
    cursor: grabbing;
  }

  /* A via place is a rounded square, as it is on the spine of the itinerary. */
  .place-marker.via {
    border-radius: 6px;
  }

  .place-marker:hover {
    background: var(--color-text);
    box-shadow: 0 0 0 1.5px var(--color-background), 0 0 0 3px var(--color-accent);
  }

  /* The chosen place gets a ring in the accent. */
  .place-marker.selected {
    box-shadow: 0 0 0 1.5px var(--color-background), 0 0 0 3.5px var(--color-accent);
  }

  .map-tooltip {
    color: var(--color-text);
    font-size: 13px;
    line-height: 1.4;
  }

  .map-tooltip .title {
    font-weight: 700;
  }

  /* The handle shrinks with the radius on screen and fades out behind the place marker. */
  .radius-handle {
    width: 10px;
    height: 10px;
    border: 2px solid var(--color-accent);
    border-radius: 50%;
    background: var(--color-surface);
    cursor: ew-resize;
    opacity: var(--handle-scale);
    transform: scale(var(--handle-scale));
    transition:
      opacity 120ms ease,
      transform 120ms ease;
  }

  .radius-handle.inactive {
    pointer-events: none;
  }

  /* The tooltip shares the surface look of the app: a hairline border and no shadow. */
  :global(.maplibregl-popup-content) {
    padding: var(--space-2) var(--space-3);
    background: var(--color-surface);
    color: var(--color-text);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    box-shadow: none;
  }

  :global(.maplibregl-popup-tip) {
    border-top-color: var(--color-surface);
    border-bottom-color: var(--color-surface);
  }
</style>
