// Writes the station tables of the booking sites in `data/fares/` from the Trainline stations
// table that the builder downloads to `build-cache/feeds/stations.csv`. The SBB table comes from
// the Swiss timetable instead, whose station names the SBB site uses.
//
// Run `node tools/booking-site-stations.js` from the root of the repository. Each table has the
// columns code, name, latitude and longitude, where the code is what the booking site takes.

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

// Trenitalia location ids are 830000000 plus the station number after the country code 83.
write('trenitalia_stations.csv', (row) => {
  const id = row[column('trenitalia_id')];
  return /^83\d{5}$/.test(id) ? String(830000000 + Number(id.slice(2))) : null;
});

// The ÖBB shop takes ÖBB station numbers. Wien Hbf is 1290401 in the shop and 8103000 here.
const obbOverrides = { 8103000: '1290401' };
write('obb_stations.csv', (row) => {
  const id = row[column('obb_id')];
  return country(row) === 'AT' && /^8\d{6}$/.test(id) && !isBus(row) ? obbOverrides[id] || id : null;
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

// The SBB site sells French trips too and takes UIC numbers of French stations. Entries for the
// RER share a position with the main line station and are left out.
write('sbb_france_stations.csv', (row) => {
  const uic = row[column('uic')];
  const isRer = /\bRER\b/.test(row[column('name')]);
  return country(row) === 'FR' && /^87\d{5}$/.test(uic) && !isBus(row) && !isRer ? uic : null;
});
