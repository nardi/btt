# Data of the offline base map

The map shows these files when no tile of OpenFreeMap is available. See `offline-basemap.ts` for the
way the map uses them.

## `basemap.geojson`

The file contains the land, the lakes, the country borders and the rail lines of Europe and the
names of the 150 biggest cities. `tools/offline-basemap.js` writes the file. Run `node
tools/offline-basemap.js` from the root of the repository to write it again.

- The land, the lakes and the borders come from Natural Earth at the scale of 1:50 million. The rail
  lines come from Natural Earth at the scale of 1:10 million. The tool simplifies them to within
  about 1.5 kilometres of the original. This data is in the public domain.
- The rail lines are an approximation. They show the main network and many regional lines, but not
  every line. The map shows them below zoom level 8 and when the tiles are missing. From zoom level
  8 on, it shows the rail lines of the tiles of OpenFreeMap.
- The cities come from `crates/btt-model/data/cities.tsv`. That file is derived from GeoNames, which
  is licensed under CC BY 4.0.

## `glyphs`

The folder contains the glyph ranges 0-255 and 256-511 of the fonts Noto Sans Regular and Noto Sans
Bold, which cover the Latin alphabets. The files come from the font server of OpenFreeMap. Noto Sans
is licensed under the SIL Open Font License 1.1. A range that the folder does not contain is
requested from OpenFreeMap, so a name in the Greek or Cyrillic alphabet needs a network connection.