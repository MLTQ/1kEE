# weather_radio.rs

## Purpose
Adds live NOAA weather-radio streams through GWES's explicitly published API.
Stream availability comes from Icecast; geographic coordinates come from the
transmitter catalog, joined by call sign rather than inferred from an event.

## Contracts
- Only actual live stream entries are exposed; transmitter-only records are not feeds.
- Alternate provider streams keep distinct IDs/URLs and provider credits.
- Weather broadcasts remain classified as Weather, never police/fire/EMS dispatch.
- Invalid/missing coordinates remain unmapped; malformed catalogs report failure.
- Source: https://www.weatherradio.org/ (WeatherRadio API).
