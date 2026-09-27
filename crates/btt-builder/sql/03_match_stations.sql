-- Step 3: collapses platforms into stations and matches each station to the Trainline stations
-- dataset. Expects the table trainline_raw and the variables stopword_pattern,
-- match_radius_meters, same_name_radius_meters, accepted_similarity and uncertain_similarity.

-- Lowercases, removes diacritics and punctuation, and strips words such as "hbf", so
-- "Köln Hbf" and "Koeln Hauptbahnhof" compare on "koln" and "koeln".
CREATE MACRO normalized_station_name(name) AS
    trim(regexp_replace(
        regexp_replace(
            regexp_replace(strip_accents(lower(name)), '[^a-z0-9]+', ' ', 'g'),
            getvariable('stopword_pattern'), ' ', 'g'),
        ' +', ' ', 'g'));

-- Rewrites a German local transit name into the order Trainline uses. It drops a leading mode
-- such as "S" or "S+U" and moves a city in parentheses to the front, so
-- "S Ostbahnhof (Berlin)" becomes "Berlin Ostbahnhof".
CREATE MACRO city_first_station_name(name) AS
    regexp_replace(
        regexp_replace(name, '^(S\+U|S|U)\s+', ''),
        '^(.*?)\s*\(([^)]*)\)\s*$', '\2 \1');

-- Every usable Trainline row. City rows group several stations, and rows with same_as duplicate
-- another row. Rows for parts of a station, such as "München Hbf Gleis 5-10", point to the
-- whole station through parent_station_id, and a match on them counts for that station.
CREATE TABLE trainline_stations AS
WITH usable_rows AS (
    SELECT *
    FROM trainline_raw
    WHERE is_city = 'f'
      AND coalesce(same_as, '') = ''
      AND TRY_CAST(latitude AS DOUBLE) IS NOT NULL
      AND TRY_CAST(longitude AS DOUBLE) IS NOT NULL
)
SELECT CAST(coalesce(parents.id, rows.id) AS INTEGER) AS trainline_station_id,
       rows.name,
       CAST(rows.latitude AS DOUBLE) AS latitude,
       CAST(rows.longitude AS DOUBLE) AS longitude,
       nullif(rows.uic, '') AS uic,
       nullif(rows.db_id, '') AS db_id,
       nullif(coalesce(parents.time_zone, rows.time_zone), '') AS time_zone,
       normalized_station_name(rows.name) AS normalized_name
FROM usable_rows AS rows
LEFT JOIN usable_rows AS parents ON parents.id = nullif(rows.parent_station_id, '');

-- The name and position of each station a match can point to, which is never a station part.
CREATE TABLE trainline_whole_stations AS
SELECT CAST(id AS INTEGER) AS trainline_station_id,
       name,
       CAST(latitude AS DOUBLE) AS latitude,
       CAST(longitude AS DOUBLE) AS longitude,
       nullif(time_zone, '') AS time_zone
FROM trainline_raw
WHERE is_city = 'f'
  AND coalesce(same_as, '') = ''
  AND TRY_CAST(latitude AS DOUBLE) IS NOT NULL
  AND TRY_CAST(longitude AS DOUBLE) IS NOT NULL;

-- One row per station key that rail trips use. The row whose stop ID is the key, which is the
-- parent station, names and places the station. Without such a row the platforms do.
CREATE TABLE feed_stations AS
WITH used_station_keys AS (
    SELECT DISTINCT stops.feed_id, stops.station_key
    FROM feed_stop_times AS stop_times
    JOIN feed_stops AS stops USING (feed_id, stop_id)
)
SELECT stops.feed_id,
       stops.station_key,
       coalesce(any_value(stops.stop_name) FILTER (WHERE stops.stop_id = stops.station_key),
                min(stops.stop_name)) AS name,
       coalesce(any_value(stops.latitude) FILTER (WHERE stops.stop_id = stops.station_key),
                avg(stops.latitude)) AS latitude,
       coalesce(any_value(stops.longitude) FILTER (WHERE stops.stop_id = stops.station_key),
                avg(stops.longitude)) AS longitude,
       max(stops.station_code) AS station_code
FROM feed_stops AS stops
JOIN used_station_keys USING (feed_id, station_key)
WHERE stops.latitude IS NOT NULL AND stops.longitude IS NOT NULL
GROUP BY stops.feed_id, stops.station_key;

-- First pass: official station codes. A separate equality join per scheme keeps DuckDB on hash
-- joins.
CREATE TABLE code_matches AS
WITH coded_stations AS (
    SELECT stations.feed_id, stations.station_key, stations.station_code,
           feed_config.station_code_scheme
    FROM feed_stations AS stations
    JOIN feed_config USING (feed_id)
    WHERE stations.station_code IS NOT NULL AND feed_config.station_code_scheme <> 'none'
),
code_candidates AS (
    SELECT coded.feed_id, coded.station_key, trainline.trainline_station_id
    FROM coded_stations AS coded
    JOIN trainline_stations AS trainline ON trainline.uic = coded.station_code
    WHERE coded.station_code_scheme = 'uic'
    UNION ALL
    SELECT coded.feed_id, coded.station_key, trainline.trainline_station_id
    FROM coded_stations AS coded
    JOIN trainline_stations AS trainline ON trainline.db_id = coded.station_code
    WHERE coded.station_code_scheme = 'eva'
)
SELECT * FROM code_candidates
QUALIFY row_number() OVER (PARTITION BY feed_id, station_key ORDER BY trainline_station_id) = 1;

-- Second pass: distance and name, for the stations the first pass left. The margins turn the
-- larger of the two radii into degrees for a bounding box, which lets DuckDB run a range join.
CREATE TABLE unmatched_feed_stations AS
SELECT stations.*,
       normalized_station_name(stations.name) AS normalized_name,
       normalized_station_name(city_first_station_name(stations.name)) AS city_first_name,
       getvariable('same_name_radius_meters') / 111320.0 AS latitude_margin,
       getvariable('same_name_radius_meters') / 111320.0
           / greatest(cos(radians(stations.latitude)), 0.05) AS longitude_margin
FROM feed_stations AS stations
ANTI JOIN code_matches USING (feed_id, station_key);

CREATE TABLE name_distance_candidates AS
SELECT unmatched.feed_id,
       unmatched.station_key,
       unmatched.name AS feed_name,
       trainline.trainline_station_id,
       trainline.name AS trainline_name,
       haversine_meters(unmatched.latitude, unmatched.longitude,
                        trainline.latitude, trainline.longitude) AS distance_meters,
       greatest(
           jaro_winkler_similarity(unmatched.normalized_name, trainline.normalized_name),
           jaro_winkler_similarity(unmatched.city_first_name, trainline.normalized_name)
       ) AS name_similarity,
       -- One name is the other plus a qualifier, as in "Lübben" and "Lübben (Spreewald)".
       -- Jaro-Winkler scores these low when the qualifier is long.
       trainline.normalized_name <> '' AND list_bool_or(list_transform(
           [unmatched.normalized_name, unmatched.city_first_name],
           feed_name -> feed_name <> '' AND (
               starts_with(trainline.normalized_name || ' ', feed_name || ' ')
               OR starts_with(feed_name || ' ', trainline.normalized_name || ' ')))
       ) AS name_is_word_prefix,
       trainline.normalized_name <> ''
           AND trainline.normalized_name IN (unmatched.normalized_name, unmatched.city_first_name)
           AS name_is_equal
FROM unmatched_feed_stations AS unmatched
JOIN trainline_stations AS trainline
  ON trainline.latitude BETWEEN unmatched.latitude - unmatched.latitude_margin
                            AND unmatched.latitude + unmatched.latitude_margin
 AND trainline.longitude BETWEEN unmatched.longitude - unmatched.longitude_margin
                             AND unmatched.longitude + unmatched.longitude_margin
WHERE haversine_meters(unmatched.latitude, unmatched.longitude,
                       trainline.latitude, trainline.longitude)
      <= getvariable('same_name_radius_meters');

-- A similar name counts within the match radius. The same name counts further out, because
-- some Trainline stations sit a few hundred meters off, such as Leiden Centraal.
CREATE TABLE name_distance_matches AS
SELECT *
FROM name_distance_candidates
WHERE (distance_meters <= getvariable('match_radius_meters')
       AND (name_similarity >= getvariable('accepted_similarity') OR name_is_word_prefix))
   OR name_is_equal
QUALIFY row_number() OVER (PARTITION BY feed_id, station_key ORDER BY distance_meters) = 1;

-- Candidates between the two thresholds go into the report and stay unmatched.
CREATE TABLE uncertain_station_matches AS
SELECT candidates.*
FROM name_distance_candidates AS candidates
ANTI JOIN name_distance_matches USING (feed_id, station_key)
WHERE candidates.distance_meters <= getvariable('match_radius_meters')
  AND candidates.name_similarity >= getvariable('uncertain_similarity')
QUALIFY row_number() OVER (
    PARTITION BY candidates.feed_id, candidates.station_key
    ORDER BY candidates.name_similarity DESC) = 1;

CREATE TABLE station_matches AS
SELECT feed_id, station_key, trainline_station_id, 'code' AS match_kind FROM code_matches
UNION ALL
SELECT feed_id, station_key, trainline_station_id, 'name and distance' FROM name_distance_matches;

-- In feeds that share a key namespace, a match for a station key counts for that key in every
-- feed of the namespace. The lowest ID decides when feeds match a key differently.
CREATE TABLE namespaced_key_matches AS
SELECT feed_config.station_key_namespace,
       matches.station_key,
       min(matches.trainline_station_id) AS trainline_station_id
FROM station_matches AS matches
JOIN feed_config USING (feed_id)
WHERE feed_config.station_key_namespace IS NOT NULL
GROUP BY ALL;

-- Feed stations that match the same Trainline station become one bundle station. Unmatched
-- stations with the same key in one namespace become one bundle station too. Any other
-- unmatched feed station becomes a bundle station of its own.
CREATE TABLE station_groups AS
WITH resolved_matches AS (
    SELECT stations.*,
           feed_config.station_key_namespace,
           CASE WHEN feed_config.station_key_namespace IS NULL THEN matches.trainline_station_id
                ELSE namespaced.trainline_station_id END AS trainline_station_id
    FROM feed_stations AS stations
    JOIN feed_config USING (feed_id)
    LEFT JOIN station_matches AS matches USING (feed_id, station_key)
    LEFT JOIN namespaced_key_matches AS namespaced
      ON namespaced.station_key_namespace = feed_config.station_key_namespace
     AND namespaced.station_key = stations.station_key
)
SELECT feed_id,
       station_key,
       name,
       latitude,
       longitude,
       trainline_station_id,
       coalesce('trainline:' || trainline_station_id,
                'key:' || station_key_namespace || ':' || station_key,
                'feed:' || feed_id || ':' || station_key) AS group_key
FROM resolved_matches;

CREATE TABLE bundle_stations AS
SELECT CAST(row_number() OVER (ORDER BY group_key) - 1 AS INTEGER) AS bundle_station_id, *
FROM (
    SELECT station_groups.group_key,
           any_value(station_groups.trainline_station_id) AS trainline_station_id,
           coalesce(any_value(trainline.name), min(station_groups.name)) AS name,
           coalesce(any_value(trainline.latitude), avg(station_groups.latitude)) AS latitude,
           coalesce(any_value(trainline.longitude), avg(station_groups.longitude)) AS longitude,
           coalesce(any_value(trainline.time_zone), any_value(feed_config.default_time_zone),
                    'UTC') AS time_zone
    FROM station_groups
    LEFT JOIN trainline_whole_stations AS trainline USING (trainline_station_id)
    JOIN feed_config USING (feed_id)
    GROUP BY station_groups.group_key
);

CREATE TABLE feed_station_bundle_ids AS
SELECT station_groups.feed_id, station_groups.station_key, bundle_stations.bundle_station_id
FROM station_groups
JOIN bundle_stations USING (group_key);
