// Collects the prices of Snälltåget night trains, for `data/fares/snalltaget_fares.jsonl`.
//
// The site of Snälltåget gives anyone a token, and its journey search answers a station pair and a
// day with the routes of the day and the price of every bundle of a route. The bundles are
// products such as a seat, a berth in a shared compartment and a private compartment, each in
// three tariffs. The script keeps the routes that sell a night train product and writes them in
// the shape of `night_fares.jsonl`, with the prices in euro at the rate of the ECB on the day.
// Snälltåget has no pass price in its search, so the records hold none.
//
// Run `node tools/snalltaget-fares.js` from the root of the repository. It appends one JSON
// document per train to `data/fares/snalltaget_fares.jsonl` and sends one request at a time,
// with a pause between requests.

import fs from 'node:fs';

const TOKEN_URL = 'https://www.snalltaget.se/token/v2';
const SEARCH_URL = 'https://apiv2.snalltaget.se/orientation/searchjourney';
const RATE_URL = 'https://data-api.ecb.europa.eu/service/data/EXR/D.SEK.EUR.SP00.A?lastNObservations=1&format=jsondata';
const OUTPUT = 'data/fares/snalltaget_fares.jsonl';
const STATIONS = 'data/fares/snalltaget_stations.csv';
const PAUSE_MS = 1200;
const FIRST_DAY_AHEAD = 5;
const LAST_DAY_AHEAD = 170;
const STEP_DAYS = 4;
const headers = { 'user-agent': 'Mozilla/5.0 (compatible; BigTrainTrip/tools)', accept: 'application/json' };

/** Pairs of the codes of stations that night trains run between, asked in both directions. */
const pairs = [
  ['740000001', 'Berlin'], ['Malmö C', 'Berlin'], ['740000001', 'Hamburg'], ['740000001', 'Malmö C'],
  ['740000001', 'Dresden'], ['Malmö C', 'Dresden'], ['740000001', '740000115'], ['740000001', '740000308'],
  ['740000001', '740000025'], ['740000001', '740000123'], ['740000001', '740000302'], ['Malmö C', '740000115'],
];

const pause = () => new Promise((resolve) => setTimeout(resolve, PAUSE_MS));

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

/** Returns the euro per Swedish krona on the latest day of the ECB, or `null`. */
async function euroPerKrona() {
  const response = await fetch(RATE_URL, { headers });
  if (!response.ok) return null;
  const answer = await response.json();
  const series = Object.values(answer.dataSets[0].series)[0];
  const kronor = Object.values(series.observations)[0][0];
  return 1 / kronor;
}

/** Returns the kind of place and the tariff of a product family, or `null` for one that is not priced per traveller. */
function productOf(family) {
  const tariffs = { NF: 'NON-FLEX', SF: 'SEMI-FLEX', FF: 'FLEX' };
  const tariff = tariffs[family.slice(-2)];
  if (!tariff) return null;
  const base = family.slice(0, -2);
  if (base === 'SP') return { type: 'SEAT', tariff, name: 'Seat' };
  if (base === 'NTB') return { type: 'COUCHETTE', tariff, name: 'Berth in a shared compartment' };
  if (base === 'NTPC' || base === 'NTPCC') return { type: 'BERTH', tariff, name: 'Private compartment' };
  return null;
}

let token = null;
let tokenAt = 0;

/** Returns a token, fetching a new one when it is old. */
async function getToken() {
  if (token && Date.now() - tokenAt < 20 * 60 * 1000) return token;
  const response = await fetch(TOKEN_URL, { headers });
  if (!response.ok) throw new Error(`token ${response.status}`);
  token = (await response.json()).access_token;
  tokenAt = Date.now();
  return token;
}

/** Searches a station pair on a day, or returns `null` when the site does not answer. */
async function search(origin, destination, day) {
  await pause();
  const response = await fetch(SEARCH_URL, {
    method: 'POST',
    headers: { ...headers, authorization: `Bearer ${await getToken()}`, 'content-type': 'application/json', 'accept-language': 'en', origin: 'https://www.snalltaget.se' },
    body: JSON.stringify({ origin, destination, departure: day, oppositedate: null, passengers: [{ type: 'AD' }], travelWithpet: false }),
  });
  return response.ok ? response.json() : null;
}

const stations = readStations();
const rate = await euroPerKrona();
if (!rate) throw new Error('no exchange rate');
const today = new Date(new Date().toISOString().slice(0, 10));
let written = 0;
const seen = new Set();
for (const [a, b] of pairs) {
  for (const [from, to] of [[a, b], [b, a]]) {
    const first = stations.get(from);
    const second = stations.get(to);
    if (!first || !second) throw new Error(`unknown station ${from} or ${to}`);
    for (let ahead = FIRST_DAY_AHEAD; ahead <= LAST_DAY_AHEAD; ahead += STEP_DAYS) {
      const day = new Date(today.getTime() + ahead * 86400000).toISOString().slice(0, 10);
      const answer = await search(from, to, day);
      for (const route of answer?.offer?.travels?.[0]?.routes ?? []) {
        const night = (route.legs ?? []).find((leg) => /night/i.test(String(leg.serviceType?.code ?? leg.serviceType?.name ?? '')));
        const families = new Map();
        for (const bundle of route.bundles ?? []) {
          const product = productOf(bundle.productFamilyId);
          if (!product || typeof bundle.price !== 'number') continue;
          const held = families.get(product.type) ?? { type: product.type, name: product.name, prices: {}, pass: {} };
          held.prices[product.tariff] = Math.min(held.prices[product.tariff] ?? Infinity, Math.round(bundle.price * rate * 100) / 100);
          families.set(product.type, held);
        }
        // A route with no berth product is a day train or sold out of berths.
        if (!families.has('COUCHETTE') && !families.has('BERTH')) continue;
        const legs = route.legs;
        const departure = legs[0].departureStation.departureTimestamp;
        const key = `${from}:${to}:${departure}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const train = night ? `Snälltåget ${night.serviceName}` : `Snälltåget ${legs[legs.length - 1].serviceName}`;
        const record = {
          collected_on: today.toISOString().slice(0, 10),
          from: { name: first.name, latitude: first.latitude, longitude: first.longitude },
          to: { name: second.name, latitude: second.latitude, longitude: second.longitude },
          departure,
          train,
          straight_kilometres: straightKilometres(first, second),
          compartments: [...families.values()],
        };
        fs.appendFileSync(OUTPUT, `${JSON.stringify(record)}\n`);
        written += 1;
      }
    }
    console.log(`${first.name} to ${second.name}: ${written} trains so far`);
  }
}
console.log(`${written} trains written to ${OUTPUT}`);
