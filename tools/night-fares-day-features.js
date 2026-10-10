// Tests whether the days ahead, the holidays and the weekday explain the prices of the night trains
// of European Sleeper and Snälltåget better than the distance alone.
//
// For the lowest seat and the lowest couchette of each operator, the script fits a regression of
// the log price on the log distance, with and without the extra inputs, and scores it by
// cross-validation in five folds by route. The error is the factor between the estimate and the
// price, and the mean error is that of the log price. All offers are from one day of collection, so
// the days ahead cannot be told apart from the date of the departure.
//
// Run `node tools/night-fares-day-features.js` from the root of the repository.

import fs from 'node:fs';


// Solves the normal equations of a ridge-free least squares by Gauss-Jordan elimination.
function ols(rows, ys) {
  const n = rows[0].length;
  const a = Array.from({ length: n }, () => new Array(n + 1).fill(0));
  rows.forEach((x, k) => {
    for (let i = 0; i < n; i++) {
      for (let j = 0; j < n; j++) a[i][j] += x[i] * x[j];
      a[i][n] += x[i] * ys[k];
    }
  });
  for (let i = 0; i < n; i++) a[i][i] += 1e-6;
  for (let i = 0; i < n; i++) {
    let pivot = i;
    for (let r = i + 1; r < n; r++) if (Math.abs(a[r][i]) > Math.abs(a[pivot][i])) pivot = r;
    [a[i], a[pivot]] = [a[pivot], a[i]];
    for (let r = 0; r < n; r++) {
      if (r === i) continue;
      const f = a[r][i] / a[i][i];
      for (let c = i; c <= n; c++) a[r][c] -= f * a[i][c];
    }
  }
  return a.map((row, i) => row[n] / row[i]);
}

function cvError(records, features, folds = 5) {
  // Folds by route, so a route is never in the training and the test part at once.
  const routeOf = (r) => [r.from.name, r.to.name].sort().join('|');
  const hash = (s) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);
  const errors = [];
  for (let f = 0; f < folds; f++) {
    const train = records.filter((r) => hash(routeOf(r)) % folds !== f);
    const test = records.filter((r) => hash(routeOf(r)) % folds === f);
    if (!test.length || !train.length) continue;
    const w = ols(train.map(features), train.map((r) => Math.log(r.y)));
    for (const r of test) {
      const pred = features(r).reduce((s, x, i) => s + x * w[i], 0);
      errors.push(Math.abs(Math.log(r.y) - pred));
    }
  }
  errors.sort((a, b) => a - b);
  return { median: Math.exp(errors[errors.length >> 1]).toFixed(3), mean: (errors.reduce((s, e) => s + e, 0) / errors.length).toFixed(3), n: errors.length };
}

for (const [file, kinds] of [['european_sleeper_fares.jsonl', ['SEAT', 'COUCHETTE']], ['snalltaget_fares.jsonl', ['SEAT', 'COUCHETTE']]]) {
  const rows = fs.readFileSync(`data/fares/${file}`, 'utf8').trim().split('\n').map(JSON.parse);
  for (const kind of kinds) {
    const records = rows
      .map((r) => {
        const c = r.compartments.find((x) => x.type === kind);
        if (!c) return null;
        const y = Math.min(...Object.values(c.prices));
        const dep = new Date(r.departure);
        const ahead = Math.max(0, (dep - new Date(r.collected_on)) / 86400000);
        const month = dep.getUTCMonth() + 1;
        const day = dep.getUTCDate();
        const holiday = (month === 12 && day >= 18) || (month === 1 && day <= 4) ? 1 : 0;
        return { ...r, y, ahead, holiday, wd: dep.getUTCDay(), km: r.straight_kilometres * 1.3 };
      })
      .filter(Boolean);
    const base = (r) => [1, Math.log(r.km / 600)];
    const withAhead = (r) => [...base(r), 1 / (r.ahead + 7) * 7, Math.log(1 + r.ahead)];
    const withHoliday = (r) => [...base(r), r.holiday];
    const withWeekday = (r) => [...base(r), ...[0, 1, 2, 3, 4, 5].map((d) => (r.wd === d ? 1 : 0))];
    const withAll = (r) => [...withAhead(r), r.holiday, ...[0, 1, 2, 3, 4, 5].map((d) => (r.wd === d ? 1 : 0))];
    console.log(`${file} ${kind} (${records.length} trains)`);
    for (const [name, f] of [['distance only', base], ['+ days ahead', withAhead], ['+ holiday', withHoliday], ['+ weekday', withWeekday], ['+ ahead, holiday, weekday', withAll]]) {
      const e = cvError(records, f);
      console.log(`  ${name.padEnd(28)} median ${e.median}x  mean log error ${e.mean}`);
    }
  }
}
