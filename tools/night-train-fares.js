// Collects the prices of night trains from the ÖBB shop, for `data/fares/night_fares.jsonl`.
//
// For each pair of stations and each day, the script asks the shop for the evening trains and
// keeps the ones that are a single night train with couchettes or sleepers. For such a train it
// asks for the offers twice, as an adult without a discount card and as an adult with the
// Interrail Global Pass card, which is how the shop sells the reservation to a pass holder. Each
// answer lists the price of every kind of accommodation per tariff.
//
// Run `node tools/night-train-fares.js [days-ahead,...]` from the root of the repository. It
// appends one JSON document per train to `data/fares/night_fares.jsonl`, and asks the shop for
// one thing at a time with two seconds between requests.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';

const SHOP = 'https://shop.oebbtickets.at';
const OUTPUT = 'data/fares/night_fares.jsonl';
const INTERRAIL_CARD = { cardId: 2391, name: 'Interrail Globalpass', numberRequired: false, isValidated: true, isSelectable: true, isDiscountCard: false };
const PAUSE_MS = 2000;
const TOKEN_LIFETIME_MS = 3 * 60 * 1000;
const headers = {
  accept: 'application/json',
  'content-type': 'application/json',
  'user-agent': 'Mozilla/5.0 (compatible; BigTrainTrip/tools)',
  Channel: 'inet',
  Lang: 'en',
  ClientId: '2',
};

/** Station numbers of the shop for the ends of night train routes. */
const stations = {
  Wien: 1290401, Salzburg: 8100002, Innsbruck: 8100108, Graz: 8100173, Villach: 8100147,
  Feldkirch: 8100197, Bregenz: 8100090, Klagenfurt: 8100085, Zürich: 8503000, Basel: 8500010,
  Hamburg: 8002549, Berlin: 8011160, Amsterdam: 8400058, Utrecht: 8400621, Köln: 8000207,
  Düsseldorf: 8000085, München: 8000261, Hannover: 8000152, Brüssel: 8800004, Roma: 8300263,
  Venezia: 8396008, Milano: 8300046, Bologna: 8300217, Budapest: 5500003, Praha: 5400014,
  Kraków: 5100028, Zagreb: 7896001, Rimini: 8300221, Arnhem: 8400071,
};

/** Pairs of night train relations in both directions, from long to short. */
const pairs = [
  ['Wien', 'Hamburg'], ['Wien', 'Zürich'], ['Wien', 'Roma'], ['Wien', 'Venezia'], ['Wien', 'Milano'],
  ['Wien', 'Berlin'], ['Wien', 'Brüssel'], ['Wien', 'Amsterdam'], ['Wien', 'Köln'], ['Wien', 'Düsseldorf'],
  ['Wien', 'Innsbruck'], ['Wien', 'München'], ['Wien', 'Feldkirch'], ['Wien', 'Bregenz'], ['Graz', 'Zürich'],
  ['Graz', 'Hamburg'], ['Salzburg', 'Hamburg'], ['Salzburg', 'Zürich'], ['Innsbruck', 'Hamburg'],
  ['Innsbruck', 'Düsseldorf'], ['Innsbruck', 'Köln'], ['Zürich', 'Hamburg'], ['Zürich', 'Berlin'],
  ['Zürich', 'Amsterdam'], ['Basel', 'Hamburg'], ['Basel', 'Berlin'], ['Basel', 'Amsterdam'],
  ['München', 'Roma'], ['München', 'Venezia'], ['München', 'Milano'], ['München', 'Budapest'],
  ['Hamburg', 'München'], ['Berlin', 'München'], ['Köln', 'München'], ['Budapest', 'Wien'],
  ['Budapest', 'Zürich'], ['Praha', 'Zürich'], ['Praha', 'Wien'], ['Kraków', 'Wien'], ['Zagreb', 'Wien'],
  ['Amsterdam', 'München'], ['Brüssel', 'Berlin'], ['Hamburg', 'Roma'], ['Berlin', 'Zürich'],
];

let session = null;

/** Waits between two requests. */
const pause = () => new Promise((resolve) => setTimeout(resolve, PAUSE_MS));

/**
 * Sends a request with `curl` and returns the parsed JSON answer, or `null` for any status but 200.
 * The shop refuses the TLS handshake of Node's own HTTP client and answers curl, as it answers the
 * app's Rust client.
 */
function curl(method, path, extraHeaders, body) {
  const arguments_ = ['-s', '-m', '60', '-w', '\n%{http_code}', '-X', method];
  for (const [name, value] of Object.entries({ ...headers, ...extraHeaders })) {
    arguments_.push('-H', `${name}: ${value}`);
  }
  if (body !== undefined) {
    arguments_.push('--data-binary', '@-');
  }
  arguments_.push(`${SHOP}${path}`);
  const output = execFileSync('curl', arguments_, { input: body === undefined ? undefined : JSON.stringify(body), maxBuffer: 1 << 26 }).toString();
  const split = output.lastIndexOf('\n');
  if (output.slice(split + 1) !== '200') {
    return null;
  }
  return JSON.parse(output.slice(0, split));
}

/** Opens a session with an anonymous token, or reuses the one that is still good. */
function openSession() {
  if (session && Date.now() - session.opened < TOKEN_LIFETIME_MS) {
    return session.token;
  }
  const token = curl('GET', '/api/domain/v1/anonymousToken', {}).access_token;
  curl('POST', '/api/domain/v1/initUserData', { AccessToken: token }, {});
  session = { token, opened: Date.now() };
  return token;
}

/** Sends one request to the shop and returns the JSON answer, or `null` when it fails. */
async function send(path, body) {
  await pause();
  const token = openSession();
  return curl('POST', path, { AccessToken: token }, body);
}

/** Returns the passengers of a request, with the Interrail card when `withPass` is set. */
const passengers = (withPass) => [{
  me: false, remembered: false, cards: withPass ? [INTERRAIL_CARD] : [], relations: [],
  challengedFlags: { hasHandicappedPass: false, hasAssistanceDog: false, hasWheelchair: false, hasAttendant: false },
  id: 1, type: 'ADULT',
}];

/** Returns the distance between two coordinates in kilometres. */
function haversine(a, b) {
  const radians = (degrees) => (degrees * Math.PI) / 180;
  const dLat = radians(b.latitude - a.latitude);
  const dLon = radians(b.longitude - a.longitude);
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(radians(a.latitude)) * Math.cos(radians(b.latitude)) * Math.sin(dLon / 2) ** 2;
  return 2 * 6371 * Math.asin(Math.sqrt(h));
}

/** Returns the offers of one connection as `{ compartments, tariffs }`, or `null` on failure. */
async function offers(connectionId, withPass) {
  const answer = await send('/api/offer/v6/offers', {
    selection: { connectionId },
    passengers: passengers(withPass),
    datetime: new Date().toISOString().slice(0, 19),
  });
  const section = answer?.offerSections?.[0];
  if (!section) {
    return null;
  }
  const kinds = new Map(section.media.compartments.map((compartment) => [compartment.id, compartment]));
  const result = {};
  for (const offer of section.travelClasses[0]?.offers ?? []) {
    const tariff = offer.flexibility?.en ?? 'UNKNOWN';
    for (const compartment of offer.reservation?.compartments ?? []) {
      const kind = kinds.get(compartment.id);
      result[compartment.id] ??= { name: kind?.name?.en ?? '', type: kind?.accommodationType ?? '', group: kind?.compartmentGroup ?? '', prices: {} };
      result[compartment.id].prices[tariff] = compartment.price;
    }
  }
  return result;
}

/** Collects the night trains between two stations that leave on a day. */
async function collect(from, to, day) {
  const entry = await send('/api/hafas/v2/externalEntrypoint', {
    cref: 'oebb-startseite',
    stations: { from: { number: stations[from] }, to: { number: stations[to] } },
  });
  if (!entry?.from || !entry?.to) {
    return 0;
  }
  const timetable = await send('/api/hafas/v4/timetable', {
    from: entry.from,
    to: entry.to,
    datetimeDeparture: `${day}T17:00:00.000`,
    filter: { regionaltrains: false, direct: true, wheelchair: false, bikes: false, trains: false, motorail: false, connections: [] },
    passengers: passengers(false),
    count: 6,
    sortType: 'DEPARTURE',
  });
  let kept = 0;
  const seen = new Set();
  for (const connection of timetable?.connections ?? []) {
    const section = connection.sections?.[0];
    const name = section?.category?.name;
    if (connection.sections.length !== 1 || !['NJ', 'EN'].includes(name)) {
      continue;
    }
    const departure = connection.from.departure;
    const key = `${section.category.number}|${departure}`;
    if (seen.has(key) || !departure.startsWith(day)) {
      continue;
    }
    seen.add(key);
    const plain = await offers(connection.id, false);
    if (!plain || !Object.values(plain).some((compartment) => ['COUCHETTE', 'BERTH'].includes(compartment.type))) {
      continue;
    }
    const pass = await offers(connection.id, true);
    const compartments = Object.entries(plain).map(([id, compartment]) => ({
      id: Number(id), ...compartment, pass: pass?.[id]?.prices ?? {},
    }));
    const document = {
      collected_on: new Date().toISOString().slice(0, 10),
      from: { name: entry.from.name, number: entry.from.number, latitude: entry.from.latitude / 1e6, longitude: entry.from.longitude / 1e6 },
      to: { name: entry.to.name, number: entry.to.number, latitude: entry.to.latitude / 1e6, longitude: entry.to.longitude / 1e6 },
      departure,
      arrival: connection.to.arrival,
      train: `${name} ${section.category.number}`,
      straight_kilometres: Math.round(haversine(
        { latitude: entry.from.latitude / 1e6, longitude: entry.from.longitude / 1e6 },
        { latitude: entry.to.latitude / 1e6, longitude: entry.to.longitude / 1e6 },
      )),
      compartments,
    };
    fs.appendFileSync(OUTPUT, JSON.stringify(document) + '\n');
    kept += 1;
  }
  return kept;
}

const daysAhead = (process.argv[2] ?? '10,24,45').split(',').map(Number);
const today = new Date();
let total = 0;
for (const offset of daysAhead) {
  const day = new Date(today.getTime() + offset * 86400000).toISOString().slice(0, 10);
  for (const [a, b] of pairs) {
    for (const [from, to] of [[a, b], [b, a]]) {
      try {
        const kept = await collect(from, to, day);
        total += kept;
        console.log(day, from, '->', to, kept, 'trains');
      } catch (error) {
        console.log(day, from, '->', to, 'failed', error.cause?.code ?? error.message);
      }
    }
  }
}
console.log('collected', total, 'trains');
