// Writes the alphabet of the compact link, for `crates/btt-model/data/compact-alphabet.txt`.
//
// A compact link writes its bytes as one big number in an alphabet of 44,190 letters of the
// Basic Multilingual Plane, each of which is one UTF-16 unit and carries 15.4 bits. The alphabet
// holds the letters (Unicode category Lo, which has no case) that stay the same under NFC and
// NFKC normalization, that are not invisible, that are not emoji, and that do not belong to a
// script written from right to left, which a display reorders around other text.
//
// The file lists the ranges of the alphabet in order, as two hexadecimal code points per line.
// The Rust encoder and the web viewer read the same file. A link depends on the exact alphabet,
// so the file never changes once links exist. Run `node tools/compact-alphabet.js <file>` from
// the root of the repository only to start a new alphabet under a new marker.

import fs from 'node:fs';

const [output] = process.argv.slice(2);
if (output === undefined) {
  console.error('usage: node tools/compact-alphabet.js <alphabet file>');
  process.exit(1);
}

const RIGHT_TO_LEFT = /\p{Script=Arabic}|\p{Script=Hebrew}|\p{Script=Syriac}|\p{Script=Thaana}|\p{Script=Nko}|\p{Script=Samaritan}|\p{Script=Mandaic}|\p{Script=Adlam}|\p{Script=Hanifi_Rohingya}|\p{Script=Phoenician}|\p{Script=Imperial_Aramaic}|\p{Script=Old_South_Arabian}|\p{Script=Old_North_Arabian}|\p{Script=Avestan}|\p{Script=Lydian}|\p{Script=Cypriot}|\p{Script=Nabataean}|\p{Script=Palmyrene}|\p{Script=Hatran}|\p{Script=Manichaean}|\p{Script=Mende_Kikakui}|\p{Script=Psalter_Pahlavi}|\p{Script=Inscriptional_Pahlavi}|\p{Script=Inscriptional_Parthian}|\p{Script=Old_Turkic}|\p{Script=Old_Hungarian}|\p{Script=Elymaic}|\p{Script=Chorasmian}|\p{Script=Sogdian}|\p{Script=Old_Sogdian}|\p{Script=Meroitic_Cursive}|\p{Script=Meroitic_Hieroglyphs}|\p{Script=Kharoshthi}|\p{Script=Yezidi}|\p{Script=Garay}/u;

/** Returns whether a character may be a digit of the alphabet. */
function isDigit(character) {
  return (
    /\p{Lo}/u.test(character) &&
    character.normalize('NFC') === character &&
    character.normalize('NFKC') === character &&
    !/\p{Default_Ignorable_Code_Point}/u.test(character) &&
    !/\p{Emoji}/u.test(character) &&
    !RIGHT_TO_LEFT.test(character)
  );
}

const ranges = [];
let count = 0;
for (let code = 0x80; code <= 0xffff; code += 1) {
  // The surrogates are not characters, and a lone one cannot travel in a URL.
  if (code >= 0xd800 && code <= 0xdfff) {
    continue;
  }
  if (!isDigit(String.fromCodePoint(code))) {
    continue;
  }
  count += 1;
  const last = ranges.at(-1);
  if (last !== undefined && last[1] === code - 1) {
    last[1] = code;
  } else {
    ranges.push([code, code]);
  }
}

const hex = (code) => code.toString(16).toUpperCase().padStart(4, '0');
const lines = [
  `# The alphabet of the compact link: ${count} letters of the Basic Multilingual Plane.`,
  `# Written by tools/compact-alphabet.js with Unicode ${process.versions.unicode}. Never edit it.`,
  ...ranges.map(([first, last]) => `${hex(first)}-${hex(last)}`),
];
fs.writeFileSync(output, `${lines.join('\n')}\n`);
console.log(`${count} letters in ${ranges.length} ranges`);
