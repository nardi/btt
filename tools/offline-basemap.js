// Writes the data of the offline base map, for `ui/shared/src/map-data/basemap.geojson`.
//
// The land, lakes and country borders come from Natural Earth at the scale of 1:50 million, which
// is in the public domain. The script cuts them to the box below, drops digits that a map at zoom
// 7 cannot show, and adds the biggest cities of `crates/btt-model/data/cities.tsv` as points. The
// rail lines come from Natural Earth at the scale of 1:10 million and are simplified further.
// Run `node tools/offline-basemap.js` from the root of the repository.
//
// Every feature has a `kind` property. The values are `land`, `lake`, `border`, `rail` and `city`. A city
// also has a `name` and a `rank`, which is its place in the list of cities by population.

import fs from 'node:fs';

const NATURAL_EARTH =
  'https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/';
const OUTPUT = 'ui/shared/src/map-data/basemap.geojson';
const CITIES = 'crates/btt-model/data/cities.tsv';

// The box that the data covers, as west, south, east and north in degrees. It holds the
// countries of the timetable, from Iceland and Portugal to Turkey.
const BOX = { west: -25, south: 34, east: 45, north: 72 };
// The number of decimals of a coordinate. Two decimals are about one kilometre.
const DECIMALS = 2;
const CITY_COUNT = 150;
// The distance in degrees that a simplified rail line can differ from the original. It is about
// 1.5 kilometres.
const RAIL_TOLERANCE = 0.015;

/** Returns the coordinate rounded to the decimals of the file. */
function round(value) {
  return Number(value.toFixed(DECIMALS));
}

/** Returns the part of the ring on the inner side of one edge of the box, in the manner of Sutherland and Hodgman. */
function clipRingAtEdge(ring, inside, intersect) {
  const output = [];
  for (let index = 0; index < ring.length; index += 1) {
    const current = ring[index];
    const previous = ring[(index + ring.length - 1) % ring.length];
    if (inside(current)) {
      if (!inside(previous)) {
        output.push(intersect(previous, current));
      }
      output.push(current);
    } else if (inside(previous)) {
      output.push(intersect(previous, current));
    }
  }
  return output;
}

/** Returns the ring cut to the box, or null when nothing of it is left. */
function clipRing(ring) {
  // The closing point repeats the first one, which the algorithm does not need.
  let points = ring.slice(0, -1);
  const edges = [
    [(p) => p[0] >= BOX.west, (a, b) => [BOX.west, a[1] + ((b[1] - a[1]) * (BOX.west - a[0])) / (b[0] - a[0])]],
    [(p) => p[0] <= BOX.east, (a, b) => [BOX.east, a[1] + ((b[1] - a[1]) * (BOX.east - a[0])) / (b[0] - a[0])]],
    [(p) => p[1] >= BOX.south, (a, b) => [a[0] + ((b[0] - a[0]) * (BOX.south - a[1])) / (b[1] - a[1]), BOX.south]],
    [(p) => p[1] <= BOX.north, (a, b) => [a[0] + ((b[0] - a[0]) * (BOX.north - a[1])) / (b[1] - a[1]), BOX.north]],
  ];
  for (const [inside, intersect] of edges) {
    if (points.length === 0) {
      return null;
    }
    points = clipRingAtEdge(points, inside, intersect);
  }
  const rounded = [];
  for (const [longitude, latitude] of points) {
    const point = [round(longitude), round(latitude)];
    const last = rounded[rounded.length - 1];
    if (last === undefined || last[0] !== point[0] || last[1] !== point[1]) {
      rounded.push(point);
    }
  }
  if (rounded.length < 3) {
    return null;
  }
  rounded.push(rounded[0]);
  return rounded;
}

/** Returns the polygon cut to the box as a list of rings, or null when nothing is left. */
function clipPolygon(rings) {
  const outer = clipRing(rings[0]);
  if (outer === null) {
    return null;
  }
  const holes = rings.slice(1).map(clipRing).filter((ring) => ring !== null);
  return [outer, ...holes];
}

/** Returns the part of the segment inside the box, in the manner of Liang and Barsky, or null. */
function clipSegment(start, end) {
  let low = 0;
  let high = 1;
  const delta = [end[0] - start[0], end[1] - start[1]];
  const limits = [
    [-delta[0], start[0] - BOX.west],
    [delta[0], BOX.east - start[0]],
    [-delta[1], start[1] - BOX.south],
    [delta[1], BOX.north - start[1]],
  ];
  for (const [direction, distance] of limits) {
    if (direction === 0) {
      if (distance < 0) {
        return null;
      }
    } else {
      const fraction = distance / direction;
      if (direction < 0) {
        low = Math.max(low, fraction);
      } else {
        high = Math.min(high, fraction);
      }
    }
  }
  if (low > high) {
    return null;
  }
  return [
    [start[0] + low * delta[0], start[1] + low * delta[1]],
    [start[0] + high * delta[0], start[1] + high * delta[1]],
  ];
}

/** Returns the line cut to the box as a list of lines, because a line can leave the box and come back. */
function clipLine(points) {
  const lines = [];
  let current = [];
  const flush = () => {
    if (current.length >= 2) {
      lines.push(current);
    }
    current = [];
  };
  for (let index = 1; index < points.length; index += 1) {
    const piece = clipSegment(points[index - 1], points[index]);
    if (piece === null) {
      flush();
      continue;
    }
    const [from, to] = piece.map(([longitude, latitude]) => [round(longitude), round(latitude)]);
    const last = current[current.length - 1];
    if (last !== undefined && (last[0] !== from[0] || last[1] !== from[1])) {
      flush();
    }
    if (current.length === 0) {
      current.push(from);
    }
    if (to[0] !== current[current.length - 1][0] || to[1] !== current[current.length - 1][1]) {
      current.push(to);
    }
  }
  flush();
  return lines;
}

/** Returns the line with fewer points, such that no dropped point is further than the tolerance from it. */
function simplifyLine(points, tolerance) {
  if (points.length < 3) {
    return points;
  }
  const [start, end] = [points[0], points[points.length - 1]];
  const length = Math.hypot(end[0] - start[0], end[1] - start[1]);
  let farthest = -1;
  let farthestDistance = 0;
  for (let index = 1; index < points.length - 1; index += 1) {
    const [x, y] = points[index];
    const distance =
      length === 0
        ? Math.hypot(x - start[0], y - start[1])
        : Math.abs((end[0] - start[0]) * (y - start[1]) - (end[1] - start[1]) * (x - start[0])) / length;
    if (distance > farthestDistance) {
      farthest = index;
      farthestDistance = distance;
    }
  }
  if (farthestDistance <= tolerance) {
    return [start, end];
  }
  const before = simplifyLine(points.slice(0, farthest + 1), tolerance);
  const after = simplifyLine(points.slice(farthest), tolerance);
  return [...before, ...after.slice(1)];
}

/** Downloads one Natural Earth file and returns its features. */
async function features(name) {
  const response = await fetch(`${NATURAL_EARTH}${name}.geojson`);
  if (!response.ok) {
    throw new Error(`${name}: ${response.status}`);
  }
  return (await response.json()).features;
}

/** Returns the polygons of a feature as lists of rings. */
function polygonsOf(geometry) {
  return geometry.type === 'Polygon' ? [geometry.coordinates] : geometry.coordinates;
}

/** Returns the lines of a feature as lists of points. */
function linesOf(geometry) {
  return geometry.type === 'LineString' ? [geometry.coordinates] : geometry.coordinates;
}

/** Returns the features of polygons cut to the box. */
function polygonFeatures(source, kind) {
  const output = [];
  for (const feature of source) {
    const polygons = polygonsOf(feature.geometry).map(clipPolygon).filter((polygon) => polygon !== null);
    if (polygons.length > 0) {
      output.push({
        type: 'Feature',
        properties: { kind },
        geometry: { type: 'MultiPolygon', coordinates: polygons },
      });
    }
  }
  return output;
}

/** Returns the cities with the biggest population, once for each position. */
function cityFeatures() {
  const seen = new Set();
  const output = [];
  for (const line of fs.readFileSync(CITIES, 'utf8').split('\n')) {
    const [name, latitudeText, longitudeText] = line.split('\t');
    if (name === undefined || longitudeText === undefined) {
      continue;
    }
    const latitude = Number(latitudeText) / 1e6;
    const longitude = Number(longitudeText) / 1e6;
    const inBox =
      longitude >= BOX.west && longitude <= BOX.east && latitude >= BOX.south && latitude <= BOX.north;
    const key = `${round(latitude)},${round(longitude)}`;
    if (!inBox || seen.has(key)) {
      continue;
    }
    seen.add(key);
    output.push({
      type: 'Feature',
      properties: { kind: 'city', name, rank: output.length },
      geometry: { type: 'Point', coordinates: [round(longitude), round(latitude)] },
    });
    if (output.length === CITY_COUNT) {
      break;
    }
  }
  return output;
}

const land = polygonFeatures(await features('ne_50m_land'), 'land');
const lakes = polygonFeatures(await features('ne_50m_lakes'), 'lake');
const borders = [];
for (const feature of await features('ne_50m_admin_0_boundary_lines_land')) {
  const lines = linesOf(feature.geometry).flatMap(clipLine);
  if (lines.length > 0) {
    borders.push({
      type: 'Feature',
      properties: { kind: 'border' },
      geometry: { type: 'MultiLineString', coordinates: lines },
    });
  }
}

// A rail line that is cut to the box and simplified can end up with a single point. All lines
// are in one feature, because the overhead of a feature is bigger than a short line.
const railLines = [];
for (const feature of await features('ne_10m_railroads')) {
  for (const line of linesOf(feature.geometry).flatMap(clipLine)) {
    const simplified = simplifyLine(line, RAIL_TOLERANCE).map(([x, y]) => [round(x), round(y)]);
    if (simplified.length >= 2) {
      railLines.push(simplified);
    }
  }
}
const rails = [
  { type: 'Feature', properties: { kind: 'rail' }, geometry: { type: 'MultiLineString', coordinates: railLines } },
];

fs.mkdirSync('ui/shared/src/map-data', { recursive: true });
fs.writeFileSync(
  OUTPUT,
  JSON.stringify({ type: 'FeatureCollection', features: [...land, ...lakes, ...borders, ...rails, ...cityFeatures()] }),
);
console.log(`${OUTPUT}: ${fs.statSync(OUTPUT).size} bytes`);
