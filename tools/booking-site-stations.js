// Writes the station tables of the booking sites in `data/fares/` from the Trainline stations
// table that the builder downloads to `build-cache/feeds/stations.csv`. The SBB table comes from
// the Swiss timetable instead, whose station names the SBB site uses.
//
// Run `node tools/booking-site-stations.js` from the root of the repository. Each table has the
// columns code, name, latitude and longitude, where the code is what the booking site takes.

import { execSync } from 'node:child_process';
import fs from 'node:fs';

const lines = fs.readFileSync('build-cache/feeds/stations.csv', 'utf8').split('\n');
const header = lines[0].split(';');
const column = (name) => header.indexOf(name);
const rows = lines.slice(1).filter(Boolean).map((line) => line.split(';'));

/** Writes one table, with the code that `pick` returns for each row, or no row for `null`. */
function write(file, pick) {
  const out = ['code,name,latitude,longitude'];
  const seen = new Set();
  for (const row of rows) {
    const code = pick(row);
    const latitude = row[column('latitude')];
    const longitude = row[column('longitude')];
    if (!code || !latitude || !longitude || seen.has(code)) {
      continue;
    }
    seen.add(code);
    const name = row[column('name')].replace(/,/g, ' ');
    out.push([code, name, Number(latitude).toFixed(5), Number(longitude).toFixed(5)].join(','));
  }
  fs.writeFileSync(`data/fares/${file}`, out.join('\n') + '\n');
  console.log(file, out.length - 1);
}

const country = (row) => row[column('country')];
const isBus = (row) => /bus/i.test(row[column('name')]);

// The DB site takes EVA numbers, which the table has for stations in many countries.
write('db_stations.csv', (row) => {
  const eva = row[column('db_id')];
  return /^\d{7}$/.test(eva) ? eva : null;
});

// A Trenitalia location id is the two-digit country code times 10 million plus the station
// number after it, such as 830008409 for 8308409 and 850003000 for 8503000. Trenitalia sells
// trips from Italy abroad, so the table has the foreign stations it knows too.
write('trenitalia_stations.csv', (row) => {
  const id = row[column('trenitalia_id')];
  return /^\d{7}$/.test(id) && !isBus(row)
    ? String(Number(id.slice(0, 2)) * 10_000_000 + Number(id.slice(2)))
    : null;
});

// The ÖBB shop takes ÖBB station numbers, also for the foreign stations it sells trips to. Wien
// Hbf is 1290401 in the shop and 8103000 here.
const obbOverrides = { 8103000: '1290401' };
write('obb_stations.csv', (row) => {
  const id = row[column('obb_id')];
  return /^\d{7}$/.test(id) && !isBus(row) ? obbOverrides[id] || id : null;
});

// PKP Intercity takes the same numbers as DB for Polish stations. Numbers from 5196000 are towns
// at the position of their main station, so they are left out.
write('pkp_stations.csv', (row) => {
  const id = row[column('db_id')];
  return country(row) === 'PL' && /^51\d{5}$/.test(id) && !/^5196/.test(id) && !isBus(row) ? id : null;
});

// NS takes UIC numbers of Dutch stations.
write('ns_stations.csv', (row) => {
  const uic = row[column('uic')];
  return country(row) === 'NL' && /^84\d{5}$/.test(uic) && !isBus(row) ? uic : null;
});

// The SBB site sells French trips, iryo trips in Spain and trips from Switzerland to its
// neighbours, and takes UIC numbers of the stations there. Entries for the RER share a position
// with the main line station and are left out.
const sbbAbroadPrefixes = { FR: '87', IT: '83', AT: '81', DE: '80', ES: '71' };
write('sbb_abroad_stations.csv', (row) => {
  const uic = row[column('uic')];
  const prefix = sbbAbroadPrefixes[country(row)];
  const isRer = /\bRER\b/.test(row[column('name')]);
  return prefix && uic.length === 7 && uic.startsWith(prefix) && /^\d+$/.test(uic) && !isBus(row) && !isRer
    ? uic
    : null;
});

// DSB takes UIC numbers of Danish stations.
write('dsb_stations.csv', (row) => {
  const uic = row[column('uic')];
  return country(row) === 'DK' && /^86\d{5}$/.test(uic) && !isBus(row) ? uic : null;
});

// Renfe takes its own five-digit station codes. Codes of letters stand for cities and are left
// out.
write('renfe_stations.csv', (row) => {
  const id = row[column('renfe_id')];
  return country(row) === 'ES' && /^\d{5}$/.test(id) && !isBus(row) ? id : null;
});

/** Writes one table from rows of code, name, latitude and longitude. */
function writeRows(file, entries) {
  const out = ['code,name,latitude,longitude'];
  for (const entry of entries) {
    out.push([entry.code, entry.name.replace(/,/g, ' '), entry.latitude.toFixed(5), entry.longitude.toFixed(5)].join(','));
  }
  fs.writeFileSync(`data/fares/${file}`, out.join('\n') + '\n');
  console.log(file, out.length - 1);
}

// SJ and Snälltåget take the stop ids of Samtrafiken, which the Swedish timetable feed uses.
// Railway stations have ids below 740010000 and names such as `Alvesta station` or
// `Stockholm Centralstation`.
const swedishStops = execSync('unzip -p build-cache/feeds/se_Trafiklab.gtfs.zip stops.txt', {
  maxBuffer: 1 << 28,
})
  .toString('utf8')
  .split('\n')
  .slice(1)
  .map((line) => /^("([^"]*)"|[^,]*),(\d+),([-\d.]+),([-\d.]+)/.exec(line))
  .filter(Boolean)
  .map((match) => ({
    name: match[2] ?? match[1],
    code: match[3],
    latitude: Number(match[4]),
    longitude: Number(match[5]),
  }));
const swedishStationsById = new Map(swedishStops.map((stop) => [stop.code, stop]));
writeRows(
  'sj_stations.csv',
  swedishStops.filter(
    (stop) =>
      Number(stop.code) < 740010000 &&
      /(station|Centralstation| C)$/i.test(stop.name) &&
      !/busstation$/i.test(stop.name),
  ),
);

/** Returns the Trainline station of a name in a country, or `undefined`. */
function trainlineStation(name, countryCode) {
  const row = rows.find((candidate) => candidate[column('name')] === name && country(candidate) === countryCode);
  return row && {
    name,
    latitude: Number(row[column('latitude')]),
    longitude: Number(row[column('longitude')]),
  };
}

/** Runs the making of a table from a site, and keeps the old table when the site fails. */
async function keepOnFailure(file, make) {
  try {
    await make();
  } catch (error) {
    console.log(file, 'kept, because the site failed:', error.cause?.code ?? error.message);
  }
}

// The sites answer clients that name themselves like the app does.
const userAgent = 'Mozilla/5.0 (compatible; BigTrainTrip/tools)';

// Snälltåget lists its stops with codes that are Samtrafiken ids in Sweden and names or ids of
// its own elsewhere. Stops abroad take their position from the Trainline station of the name
// here.
const snalltagetNames = {
  Berlin: ['Berlin Hbf', 'DE'],
  Hamburg: ['Hamburg Hbf', 'DE'],
  Dresden: ['Dresden Hbf', 'DE'],
  Köpenhamn: ['København', 'DK'],
  'Malmö C': ['Malmö Central', 'SE'],
  860000083: ['Kolding St.', 'DK'],
  860000512: ['Odense St.', 'DK'],
  860000100: ['Padborg St.', 'DK'],
  800024643: ['Neumünster', 'DE'],
  810000462: ['Salzburg Hbf', 'AT'],
  810000522: ['Innsbruck Hbf', 'AT'],
};
await keepOnFailure('snalltaget_stations.csv', async () => {
const snalltagetToken = await (await fetch('https://www.snalltaget.se/token/v2', { headers: { accept: 'application/json', 'user-agent': userAgent } })).json();
const snalltagetStops = await (
  await fetch('https://apiv2.snalltaget.se/navigation/stops', {
    headers: { accept: 'application/json', 'user-agent': userAgent, authorization: `Bearer ${snalltagetToken.access_token}`, origin: 'https://www.snalltaget.se' },
  })
).json();
writeRows(
  'snalltaget_stations.csv',
  snalltagetStops.items.flatMap((stop) => {
    const swedish = swedishStationsById.get(stop.code);
    if (swedish && /(station|Centralstation| C)$/i.test(swedish.name) && !/(busstation|bstn)$/i.test(stop.name)) {
      return [{ ...swedish, name: stop.name }];
    }
    const [name, countryCode] = snalltagetNames[stop.code] ?? [];
    const station = name && trainlineStation(name, countryCode);
    return station ? [{ ...station, code: stop.code, name: stop.name }] : [];
  }),
);
});

// Eurostar takes UIC numbers, and its own list names the stations its trains call at.
await keepOnFailure('eurostar_stations.csv', async () => {
const eurostarStations = await (
  await fetch('https://site-api.eurostar.com/search/stations/graphql', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'user-agent': userAgent,
      origin: 'https://www.eurostar.com',
      // The key of the site's own pages.
      'x-api-key': 'NGktEpCX5R2jYamA9WejQ5b5ryxxUhq51pg7iNXm',
    },
    body: JSON.stringify({
      operationName: 'getStations',
      variables: { withDefaultConnections: true },
      query: 'query getStations($withDefaultConnections: Boolean) { stations(withDefaultConnections: $withDefaultConnections) { uic name isEurostarDirect isErDirect } }',
    }),
  })
).json();
const eurostarCodes = new Set(
  eurostarStations.data.stations.filter((station) => station.isEurostarDirect || station.isErDirect).map((station) => station.uic),
);
write('eurostar_stations.csv', (row) => {
  const uic = row[column('uic')];
  return eurostarCodes.has(uic) && !isBus(row) ? uic : null;
});
});

// MÁV takes the UIC numbers of Hungarian stations with two leading zeros. Its station list names
// each station's number in the MÁV timetable feed, whose stops have the positions.
await keepOnFailure('mav_stations.csv', async () => {
const mavPositions = new Map(
  execSync('unzip -p build-cache/feeds/hu_mav.gtfs.zip stops.txt', { maxBuffer: 1 << 28 })
    .toString('utf8')
    .split('\n')
    .slice(1)
    .map((line) => /^("([^"]*)"|[^,]*),([^,]+),([-\d.]+),([-\d.]+)/.exec(line))
    .filter(Boolean)
    .map((match) => [match[3], { latitude: Number(match[4]), longitude: Number(match[5]) }]),
);
const mavList = await (
  await fetch('https://jegy-a.mav.hu/IK_API_PROD/api/OfferRequestApi/GetStationList', {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'user-agent': userAgent, language: 'en', origin: 'https://jegy.mav.hu' },
    body: '{}',
  })
).json();
writeRows(
  'mav_stations.csv',
  mavList.stations
    .filter((station) => station.coutryIso === 'HU' && !station.isAlias && /^0055\d{5}$/.test(station.code))
    .flatMap((station) => {
      const position = mavPositions.get(station.baseCode);
      return position ? [{ code: station.code, name: station.name, ...position }] : [];
    }),
);
});

// RegioJet lists its stations with positions in its open API. Only train stations are kept.
await keepOnFailure('regiojet_stations.csv', async () => {
const regiojetCountries = await (
  await fetch('https://brn-ybus-pubapi.sa.cz/restapi/consts/locations', {
    headers: { accept: 'application/json', 'user-agent': userAgent, 'x-lang': 'en' },
  })
).json();
writeRows(
  'regiojet_stations.csv',
  regiojetCountries.flatMap((country) =>
    country.cities.flatMap((city) =>
      city.stations
        .filter((station) => (station.stationsTypes ?? []).includes('TRAIN_STATION') && station.latitude && station.longitude)
        .map((station) => ({
          code: String(station.id),
          name: station.fullname ?? station.name,
          latitude: Number(station.latitude),
          longitude: Number(station.longitude),
        })),
    ),
  ),
);
});
