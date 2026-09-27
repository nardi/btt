-- Typed tables that every feed loads into. Each row carries the feed it came from, so IDs of
-- different feeds never collide.

CREATE TABLE feed_config (
    feed_id VARCHAR PRIMARY KEY,
    station_code_scheme VARCHAR NOT NULL,
    default_time_zone VARCHAR,
    -- Feeds with the same namespace share station keys, so equal keys are the same station.
    station_key_namespace VARCHAR
);

CREATE TABLE feed_agencies (
    feed_id VARCHAR NOT NULL,
    agency_id VARCHAR,
    agency_name VARCHAR,
    agency_timezone VARCHAR
);

CREATE TABLE feed_routes (
    feed_id VARCHAR NOT NULL,
    route_id VARCHAR NOT NULL,
    agency_id VARCHAR,
    route_short_name VARCHAR,
    route_type INTEGER NOT NULL
);

CREATE TABLE feed_trips (
    feed_id VARCHAR NOT NULL,
    trip_id VARCHAR NOT NULL,
    route_id VARCHAR NOT NULL,
    service_id VARCHAR NOT NULL,
    trip_short_name VARCHAR
);

CREATE TABLE feed_stop_times (
    feed_id VARCHAR NOT NULL,
    trip_id VARCHAR NOT NULL,
    stop_sequence INTEGER NOT NULL,
    stop_id VARCHAR NOT NULL,
    -- Seconds since noon minus 12 hours in local time on the service day, as GTFS defines.
    arrival_seconds INTEGER NOT NULL,
    departure_seconds INTEGER NOT NULL,
    boarding_allowed BOOLEAN NOT NULL,
    alighting_allowed BOOLEAN NOT NULL
);

CREATE TABLE feed_stops (
    feed_id VARCHAR NOT NULL,
    stop_id VARCHAR NOT NULL,
    stop_name VARCHAR,
    latitude DOUBLE,
    longitude DOUBLE,
    station_key VARCHAR NOT NULL,
    station_code VARCHAR
);

CREATE TABLE feed_service_days (
    feed_id VARCHAR NOT NULL,
    service_id VARCHAR NOT NULL,
    -- Day number within the bundle's validity window.
    service_day SMALLINT NOT NULL
);

-- Turns a GTFS time such as '25:10:00' into seconds. Hours can pass 24 for trips that run past
-- midnight.
CREATE MACRO gtfs_seconds(gtfs_time) AS
    CAST(split_part(trim(gtfs_time), ':', 1) AS INTEGER) * 3600
    + CAST(split_part(trim(gtfs_time), ':', 2) AS INTEGER) * 60
    + CAST(split_part(trim(gtfs_time), ':', 3) AS INTEGER);

CREATE MACRO gtfs_date(gtfs_date_text) AS CAST(strptime(trim(gtfs_date_text), '%Y%m%d') AS DATE);

CREATE MACRO haversine_meters(latitude_a, longitude_a, latitude_b, longitude_b) AS
    2 * 6371008.8 * asin(sqrt(
        pow(sin(radians(latitude_b - latitude_a) / 2), 2)
        + cos(radians(latitude_a)) * cos(radians(latitude_b))
          * pow(sin(radians(longitude_b - longitude_a) / 2), 2)));
