-- Step 4: footpaths between bundle stations close to each other. Expects the variable
-- footpath_radius_meters.

CREATE TABLE footpaths AS
WITH stations AS (
    SELECT bundle_station_id,
           latitude,
           longitude,
           getvariable('footpath_radius_meters') / 111320.0 AS latitude_margin,
           getvariable('footpath_radius_meters') / 111320.0
               / greatest(cos(radians(latitude)), 0.05) AS longitude_margin
    FROM bundle_stations
),
station_pairs AS (
    SELECT from_stations.bundle_station_id AS from_station,
           to_stations.bundle_station_id AS to_station,
           haversine_meters(from_stations.latitude, from_stations.longitude,
                            to_stations.latitude, to_stations.longitude) AS distance_meters
    FROM stations AS from_stations
    JOIN stations AS to_stations
      ON to_stations.latitude BETWEEN from_stations.latitude - from_stations.latitude_margin
                                  AND from_stations.latitude + from_stations.latitude_margin
     AND to_stations.longitude BETWEEN from_stations.longitude - from_stations.longitude_margin
                                   AND from_stations.longitude + from_stations.longitude_margin
    WHERE from_stations.bundle_station_id <> to_stations.bundle_station_id
)
-- Short distances are walks at 5 km/h along a path 30 percent longer than the straight line.
-- Longer ones assume local transit at 15 km/h plus 15 minutes of waiting and walking.
SELECT from_station,
       to_station,
       distance_meters,
       CAST(greatest(1, CASE
           WHEN distance_meters < 1000 THEN ceil(distance_meters * 1.3 / 83.3)
           ELSE ceil(15 + distance_meters / 250)
       END) AS SMALLINT) AS duration_minutes
FROM station_pairs
WHERE distance_meters <= getvariable('footpath_radius_meters');
