// Writes the dictionary of cities that links and the web viewer share, for
// `crates/btt-model/data/cities.tsv`.
//
// The cities come from the GeoNames file `cities15000.txt`, which lists the places of more than
// 15,000 people and is licensed under CC BY 4.0. Run `node tools/city-dictionary.js
// <cities15000.txt> <dictionary file> [minimum population] [folder of alternate names]` from the
// root of the repository. The folder holds the unpacked files of
// `https://download.geonames.org/export/dump/alternatenames/` for the countries below, and it
// adds the English names of the cities, such as "Cologne" for Köln, which the geocoder returns.
//
// The dictionary file holds one name per line as the name, the latitude and the longitude in
// microdegrees, separated by tabs. A link names a city by its line, so the script keeps the lines
// the file already has, in their order, and appends the names it lacks. New lines come in order of
// population, the biggest city first, which gives the cities travellers meet the shortest
// numbers. A city whose plain ASCII name differs from its name gets a line for each, because the
// geocoder sometimes returns one and sometimes the other.

import fs from 'node:fs';

// The countries that the timetable feeds cover.
const COUNTRIES = new Set([
  'AL', 'AT', 'BA', 'BE', 'BG', 'CH', 'CZ', 'DE', 'DK', 'EE', 'ES', 'FI', 'FR', 'GB', 'GR',
  'HR', 'HU', 'IE', 'IT', 'LT', 'LU', 'LV', 'ME', 'MK', 'NL', 'NO', 'PL', 'PT', 'RO', 'RS',
  'SE', 'SI', 'SK', 'TR',
]);

const [input, output, minimumPopulationText, alternateNamesFolder] = process.argv.slice(2);
if (input === undefined || output === undefined) {
  console.error('usage: node tools/city-dictionary.js <cities15000.txt> <dictionary file> [minimum population]');
  process.exit(1);
}
const minimumPopulation = Number(minimumPopulationText ?? 30000);

/** Returns the English names of places by GeoNames ID, which the files of the countries list. */
function englishNames() {
  const byId = new Map();
  if (alternateNamesFolder === undefined) {
    return byId;
  }
  for (const country of COUNTRIES) {
    const file = `${alternateNamesFolder}/${country}.txt`;
    if (!fs.existsSync(file)) {
      continue;
    }
    for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
      const fields = line.split('\t');
      // The columns are the ID of the name, the ID of the place, the language, the name and the
      // flags for preferred, short, colloquial and historic names.
      if (fields[2] !== 'en' || fields[6] === '1' || fields[7] === '1') {
        continue;
      }
      const names = byId.get(fields[1]) ?? [];
      names.push(fields[3]);
      byId.set(fields[1], names);
    }
  }
  return byId;
}

const english = englishNames();
const cities = [];
for (const line of fs.readFileSync(input, 'utf8').split('\n')) {
  const fields = line.split('\t');
  if (fields.length < 15 || !COUNTRIES.has(fields[8])) {
    continue;
  }
  const population = Number(fields[14]);
  if (population < minimumPopulation) {
    continue;
  }
  const latitude = Math.round(Number(fields[4]) * 1e6);
  const longitude = Math.round(Number(fields[5]) * 1e6);
  // The English names come first, because they are what the geocoder returns.
  const names = [...new Set([...(english.get(fields[0]) ?? []), fields[1], fields[2]])];
  cities.push({ names, latitude, longitude, population });
}
cities.sort((a, b) => b.population - a.population);

const lines = fs.existsSync(output) ? fs.readFileSync(output, 'utf8').split('\n').filter((line) => line !== '') : [];
const known = new Set(lines);
for (const city of cities) {
  for (const name of city.names) {
    // A tab or a line break would split the line.
    if (/[\t\n]/.test(name)) {
      continue;
    }
    const line = `${name}\t${city.latitude}\t${city.longitude}`;
    if (!known.has(line)) {
      known.add(line);
      lines.push(line);
    }
  }
}
fs.writeFileSync(output, `${lines.join('\n')}\n`);
console.log(`${lines.length} lines in ${output}`);
