-- Step 3b: turns the station groups into bundle stations. Expects the table
-- feed_station_groups, which assigns every feed station number to a group number.

-- Every feed station with its group and its Trainline match, if any.
CREATE TABLE station_groups AS
SELECT stations.feed_id,
       stations.station_key,
       stations.name,
       stations.latitude,
       stations.longitude,
       groups.group_number,
       matches.trainline_station_id
FROM feed_stations AS stations
JOIN feed_station_numbers AS numbers USING (feed_id, station_key)
JOIN feed_station_groups AS groups USING (feed_station_number)
LEFT JOIN station_matches AS matches USING (feed_id, station_key);

-- Feed coordinates place a bundle station, because Trainline coordinates are sometimes off by
-- a kilometer. The Trainline match only adds a name, an ID and a time zone. When members
-- match different Trainline stations, the lowest ID wins.
CREATE TABLE bundle_stations AS
WITH grouped AS (
    SELECT group_number,
           min(trainline_station_id) AS trainline_station_id,
           min(name) AS feed_name,
           avg(latitude) AS latitude,
           avg(longitude) AS longitude,
           any_value(feed_config.default_time_zone) AS feed_time_zone
    FROM station_groups
    JOIN feed_config USING (feed_id)
    GROUP BY group_number
)
SELECT CAST(row_number() OVER (ORDER BY grouped.group_number) - 1 AS INTEGER)
           AS bundle_station_id,
       grouped.group_number,
       grouped.trainline_station_id,
       coalesce(trainline.name, grouped.feed_name) AS name,
       grouped.latitude,
       grouped.longitude,
       coalesce(trainline.time_zone, grouped.feed_time_zone, 'UTC') AS time_zone
FROM grouped
LEFT JOIN trainline_whole_stations AS trainline USING (trainline_station_id);

CREATE TABLE feed_station_bundle_ids AS
SELECT station_groups.feed_id, station_groups.station_key, bundle_stations.bundle_station_id
FROM station_groups
JOIN bundle_stations USING (group_number);
