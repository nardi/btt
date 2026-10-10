// Collects the prices of European Sleeper trains, for `data/fares/european_sleeper_fares.jsonl`.
//
// The booking service of European Sleeper answers plain requests. For each pair of stations the
// script asks for the trains on days spread over the next months, and for each train it asks for
// the availability twice: as an adult, whose price holds the ticket and the place, and as an
// Interrail pass holder, who pays the reservation of the place only. The availability lists the
// lowest price of every kind of place. The records have the shape of `night_fares.jsonl`, so the
// same fit reads them.
//
// Run `node tools/european-sleeper-fares.js [trains-per-direction]` from the root of the
// repository. `ES_STEP_DAYS` sets the days between two searches of a pair and `ES_SKIP_PAIRS` the
// number of pairs to leave out from the start of the list. It appends one JSON document per train to
// `data/fares/european_sleeper_fares.jsonl` and sends one request at a time, with a pause between
// requests.

import fs from 'node:fs';

const SERVICE = 'https://europeansleeperprod-api.azurewebsites.net';
const OUTPUT = 'data/fares/european_sleeper_fares.jsonl';
const STATIONS = 'data/fares/european_sleeper_stations.csv';
const PAUSE_MS = 1200;
const ADULT = 72;
const INTERRAIL = 44;
const FIRST_DAY_AHEAD = 7;
const LAST_DAY_AHEAD = 170;
const SEARCH_STEP_DAYS = Number(process.env.ES_STEP_DAYS ?? 5);
const SKIP_PAIRS = Number(process.env.ES_SKIP_PAIRS ?? 0);
const TRAINS_PER_DIRECTION = Number(process.argv[2] ?? 14);
const headers = {
  accept: 'application/json',
  'content-type': 'application/json',
  origin: 'https://booking.europeansleeper.eu',
  'user-agent': 'Mozilla/5.0 (compatible; BigTrainTrip/tools)',
};

/** Pairs of the codes of stations that trains run between, which the script asks in both directions. */
const pairs = [
  ['8400058', '8010100'], ['8400058', '8001305'], ['8400058', '5457076'], ['8400058', '8020401'],
  ['8800104', '8010100'], ['8800104', '8001305'], ['8800104', '5457076'], ['8800104', '8020401'],
  ['8800210', '8010100'], ['8800210', '5457076'], ['8400530', '8010100'], ['8400530', '5457076'],
  ['8400055', '8010100'], ['8400173', '8010100'], ['8010100', '5457076'], ['8010100', '8001305'],
  ['8700015', '8010100'], ['8700015', '8020401'], ['8800104', '8500200'], ['8800410', '8500200'],
  ['8052101', '8300112'], ['8050500', '8300112'], ['8800104', '8300112'], ['8500200', '8300112'],
];

const pause = () => new Promise((resolve) => setTimeout(resolve, PAUSE_MS));

/** Posts a request and returns the answer, or `null` when the service does not answer with 200. */
async function post(path, body) {
  await pause();
  try {
    const response = await fetch(`${SERVICE}${path}`, { method: 'POST', headers, body: JSON.stringify(body) });
    if (!response.ok) return null;
    return await response.json();
  } catch {
    return null;
  }
}

/** Reads the stations of the CSV by code. */
function readStations() {
  const stations = new Map();
  for (const line of fs.readFileSync(STATIONS, 'utf8').split('\n').slice(1)) {
    const [code, name, latitude, longitude] = line.split(',');
    if (code) stations.set(code, { code, name, latitude: Number(latitude), longitude: Number(longitude) });
  }
  return stations;
}

/** Returns the distance in kilometres between two stations along the great circle. */
function straightKilometres(a, b) {
  const rad = (degrees) => (degrees * Math.PI) / 180;
  const h = Math.sin(rad(b.latitude - a.latitude) / 2) ** 2
    + Math.cos(rad(a.latitude)) * Math.cos(rad(b.latitude)) * Math.sin(rad(b.longitude - a.longitude) / 2) ** 2;
  return Math.round(12742 * Math.asin(Math.sqrt(h)));
}

/** Returns the kind of place of a place type key, or `null` for one that is not priced per traveller. */
function kindOf(key) {
  if (key.includes('private')) return null;
  if (key.startsWith('seat')) return 'SEAT';
  if (key.startsWith('couchette')) return 'COUCHETTE';
  if (key.startsWith('berth') || key.startsWith('comfort')) return 'BERTH';
  return null;
}

/** Turns an availability answer into the lowest price per fare type of every kind of place. */
function compartmentsOf(answer) {
  const lowest = new Map();
  for (const place of answer?.availabilityResult?.availability?.priceClasses ?? []) {
    const type = kindOf(place.placeTypeKey);
    if (!type) continue;
    for (const fare of place.fareTypes ?? []) {
      const price = fare.prices?.eur;
      if (!fare.bookable || typeof price !== 'number') continue;
      const key = `${type}`;
      const held = lowest.get(key) ?? { type, name: place.placeTypeKey, prices: {} };
      held.prices[fare.id] = Math.min(held.prices[fare.id] ?? Infinity, price);
      lowest.set(key, held);
    }
  }
  return lowest;
}

/** Collects the trains of one direction of a pair. */
async function collectDirection(from, to, stations, today, out) {
  const seen = new Map();
  for (let ahead = FIRST_DAY_AHEAD; ahead <= LAST_DAY_AHEAD; ahead += SEARCH_STEP_DAYS) {
    const day = new Date(today.getTime() + ahead * 86400000).toISOString().slice(0, 10);
    const found = await post('/api/search/trains', {
      fromLocationId: from.code, toLocationId: to.code, departureDate: `${day}T00:00:00.000Z`,
      passengerTypes: [ADULT], bicycleCount: 0, petsCount: 0, returnDate: null,
    });
    for (const route of found?.routeSearchResults ?? []) {
      for (const result of route.searchResultOutward ?? []) {
        const trip = result.trip;
        if (!trip || !trip.bookable) continue;
        seen.set(`${trip.trainNumber}:${trip.departureTime}`, { routeId: result.routeId, trip });
      }
    }
  }
  const all = [...seen.values()].sort((a, b) => a.trip.departureTime.localeCompare(b.trip.departureTime));
  const step = Math.max(1, Math.floor(all.length / TRAINS_PER_DIRECTION));
  const chosen = all.filter((_, index) => index % step === 0).slice(0, TRAINS_PER_DIRECTION);
  for (const { routeId, trip } of chosen) {
    const request = (type) => ({
      trainRouteId: routeId, fromLocationId: from.code, toLocationId: to.code, passengerTypes: [type],
      trainNumber: trip.trainNumber, travelDate: trip.travelDate, bicycleCount: 0, petsCount: 0,
      connectingTrainSelection: null,
    });
    const adult = compartmentsOf(await post('/api/search/availability', request(ADULT)));
    const pass = compartmentsOf(await post('/api/search/availability', request(INTERRAIL)));
    if (adult.size === 0) continue;
    const compartments = [...adult.values()].map((place) => ({
      type: place.type, name: place.name, prices: place.prices,
      pass: pass.get(place.type)?.prices ?? {},
    }));
    const record = {
      collected_on: today.toISOString().slice(0, 10),
      from: { name: from.name, number: Number(from.code), latitude: from.latitude, longitude: from.longitude },
      to: { name: to.name, number: Number(to.code), latitude: to.latitude, longitude: to.longitude },
      departure: trip.departureTime,
      arrival: trip.arrivalTime,
      train: `ES ${trip.trainNumber}`,
      straight_kilometres: straightKilometres(from, to),
      compartments,
    };
    fs.appendFileSync(OUTPUT, `${JSON.stringify(record)}\n`);
    out.count += 1;
  }
  console.log(`${from.name} to ${to.name}: ${all.length} trains found, ${chosen.length} read`);
}

const stations = readStations();
const today = new Date(new Date().toISOString().slice(0, 10));
const out = { count: 0 };
for (const [a, b] of pairs.slice(SKIP_PAIRS)) {
  const first = stations.get(a);
  const second = stations.get(b);
  if (!first || !second) throw new Error(`unknown station in ${a} ${b}`);
  await collectDirection(first, second, stations, today, out);
  await collectDirection(second, first, stations, today, out);
}
console.log(`${out.count} trains written to ${OUTPUT}`);
