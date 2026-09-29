<script lang="ts">
  // Map of the trip: places as markers, station search radii as circles, planned rides as lines
  // and the ways between places and stations as dotted lines. In an editable source, double
  // clicks add places and drags move them or change their radius.
  import "maplibre-gl/dist/maplibre-gl.css";

  import type { GeoCoordinates, TripEdit, TripPlace, TripPlaceId } from "../bindings";
  import type { Map as MapLibreMap, MapLayerMouseEvent, MapMouseEvent } from "maplibre-gl";
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
  import { tripPlaces } from "../itinerary";
  import {
    type PlaceDragPreview,
    type PlaceFocusRequest,
    placeBounds,
    radiusCircles,
    rideLines,
    stationWayLines,
    transferPoints,
  } from "../map-features";
  import type { TripSource } from "../trip-source";

  /** OpenFreeMap style. Needs no API key and asks for OpenStreetMap attribution, which MapLibre shows. */
  const mapStyleUrl = "https://tiles.openfreemap.org/styles/liberty";
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

  let map: MapLibreMap | undefined = $state.raw();
  let dragPreview: PlaceDragPreview | null = $state(null);
  /** Trip whose places the map last zoomed to, so edits never move the view. */
  let fittedTripId: string | null = null;
  /** Places the map has seen for the current trip, so it notices added ones. */
  let knownPlaceIds = new Set<TripPlaceId>();
  /** Last focus request the map moved for, so trip edits do not repeat the move. */
  let handledFocusRequestNumber = 0;

  const trip = $derived(source.viewState.trip);
  const places = $derived(tripPlaces(trip.itinerary));
  const editable = $derived(source.kind === "editable" ? source : null);
  const circles = $derived(radiusCircles(trip, selectedPlaceId, dragPreview));
  const rides = $derived(rideLines(source.viewState));
  const stationWays = $derived(stationWayLines(source.viewState));
  const transfers = $derived(transferPoints(source.viewState));

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
  style={mapStyleUrl}
  autoloadGlobalCss={false}
  center={[10, 50]}
  zoom={4}
  doubleClickZoom={editable === null}
  onclick={handleMapClick}
  ondblclick={handleMapDoubleClick}
  onmoveend={reportView}
>
  <GeoJSONSource id="station-search-radii" data={circles}>
    <FillLayer
      paint={{
        "fill-color": "#1f4e79",
        "fill-opacity": ["case", ["get", "selected"], 0.16, 0.07],
      }}
    />
    <LineLayer paint={{ "line-color": "#1f4e79", "line-width": 1.5, "line-opacity": 0.6 }} />
  </GeoJSONSource>

  <GeoJSONSource id="station-ways" data={stationWays}>
    <LineLayer
      layout={{ "line-cap": "round" }}
      paint={{ "line-color": "#475569", "line-width": 2, "line-dasharray": [0.5, 2] }}
    />
  </GeoJSONSource>

  <GeoJSONSource id="planned-rides" data={rides}>
    <!-- Purple marks trains with couchettes or sleepers, whatever the leg's travel mode. -->
    <LineLayer
      layout={{ "line-cap": "round" }}
      paint={{
        "line-color": ["case", ["get", "offersBerths"], "#6d28d9", "#c2410c"],
        "line-width": 3,
      }}
    />
    <!-- A wide invisible line catches the pointer, because a 3 pixel line is hard to hit. -->
    <LineLayer
      paint={{ "line-color": "#000000", "line-opacity": 0, "line-width": 14 }}
      onmousemove={showTooltip}
      onmouseleave={hideTooltip}
    />
  </GeoJSONSource>

  <GeoJSONSource id="transfer-points" data={transfers}>
    <CircleLayer
      paint={{
        "circle-radius": ["case", ["==", ["get", "kind"], "transfer"], 6, 4],
        "circle-color": "#ffffff",
        "circle-stroke-color": "#c2410c",
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
          class:selected={place.id === selectedPlaceId}
          title={place.name}
          onclick={() => onSelect(place.id)}
        >
          {index + 1}
        </button>
      {/snippet}
    </Marker>
    {#if editable !== null}
      <Marker
        lnglat={radiusHandlePosition(place)}
        draggable
        ondrag={(event) => previewRadius(place, event.target.getLngLat())}
        ondragend={() => finishRadius(place)}
      >
        {#snippet content()}
          <div class="radius-handle" title="Drag to change the station search radius"></div>
        {/snippet}
      </Marker>
    {/if}
  {/each}
</MapLibre>

<style>
  :global(.trip-map) {
    width: 100%;
    height: 100%;
  }

  .place-marker {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 2px solid #ffffff;
    border-radius: 50%;
    background: #1f4e79;
    color: #ffffff;
    font-weight: 700;
    box-shadow: 0 1px 4px rgb(0 0 0 / 40%);
  }

  .place-marker.selected {
    background: #c2410c;
  }

  .map-tooltip {
    color: #1b2430;
    font-size: 13px;
    line-height: 1.4;
  }

  .map-tooltip .title {
    font-weight: 600;
  }

  .radius-handle {
    width: 12px;
    height: 12px;
    border: 2px solid #1f4e79;
    border-radius: 50%;
    background: #ffffff;
    cursor: ew-resize;
  }
</style>
