# Nearby emergency radio

Select an event and click the **radio icon in the Brief title bar**, or **Nearby
radio** in its focused Event Queue card. The floating window searches open sources for police,
fire, EMS, public safety, aviation, marine and weather radio. Filter by type,
name/place and a 25–500 km radius. Press **Play** on a feed; Pause, Resume, Stop
and Volume control the single player. Closing the window stops audio.

The radio window stays pinned to the searched event while Follow Events moves
on. Use **Search selected event** to change areas explicitly. That stops the
previous feed. Nothing plays automatically and there is no startup radio query.

## Coverage

Sources include Radio Browser, GWES's live WeatherRadio API, 18 independent
NCISS dispatch/interoperability streams and 22 ScanBC scanner/aviation streams.
GWES weather broadcasts are labelled Weather and are separate from dispatch.
Provider/coverage varies by region. Published station or area-center coordinates are approximate and do not establish radio reception,
agency jurisdiction or a connection to the selected incident. Entries without
coordinates are excluded from nearby results. **Browse all open feeds** includes
unmapped and distant entries and labels their distance/status honestly.

Searches fetch a bounded category subset, deduplicate UUIDs and stream URLs,
and cache successful results for 15 minutes. Failed/full subqueries display an
incomplete-results message. Refresh is explicit; a failed search preserves older
results, while a partial refresh lists the sources that succeeded plus bundled feeds.
Directory listing availability is not a guarantee that an audio feed is online.

The independent list is an attributed November 6, 2026 snapshot. NCISS county
feeds use Census representative county points labelled coverage centers. ScanBC
uses bundled settlement centers credited to OpenStreetMap contributors (ODbL).
These are approximate area centers, not receiver positions. Other regional feeds
remain unmapped; Browse all still lists them. No runtime geocoding is required.

## Additional public streams

Expand **Add a public stream…**, enter its name/direct HTTP(S) audio URL and type,
then **Save at this event location**. This explicitly pins the stream to the event
coordinates; the pin is not a measured coverage area. Up to 200 feeds persist in
`.1kee_radio_feeds.json` beside the executable, separate from API credentials.
Removal is available on each saved feed card. Failed saves retain the old list.

Use streams you are permitted to play. Broadcastify/OpenMHz catalog and audio
integration require separate provider approval and are not queried or hotlinked
by this implementation. Links to their own listening sites are provided. Restricted
provider URLs found in the community directory are excluded from in-app playback.

## Playback

macOS uses native AVFoundation and requires no external player. It reports actual
connecting/playing/error states and drops stalled connections after 30 seconds.
Other desktop targets use ffplay from FFmpeg on PATH; Pause/Resume and volume
changes reconnect the live stream. Those targets report Connecting because an
external process alone cannot prove audio output. MP3/AAC/HLS support varies by
platform and provider. Audio is current provider output, including any provider
delay; it is not aligned to historical event time or replay.

Radio audio and metadata are not proxied to Gruve, recorded, transcribed or added
to event replay. An OBS desktop-audio capture may include audible radio normally.

## Sources

- [Radio Browser API and application guidance](https://api.radio-browser.info/)
- [Radio Browser endpoint reference](https://docs.radio-browser.info/)
- [GWES WeatherRadio and documented APIs](https://www.weatherradio.org/)
- [NCISS direct-stream player instructions](https://nciss.live/)
- [ScanBC public feeds](https://www.scanbc.com/)
- [Census county representative points](https://www2.census.gov/geo/docs/maps-data/data/gazetteer/2025_Gazetteer/2025_gaz_counties_19.txt)
- [OpenStreetMap attribution and licence](https://www.openstreetmap.org/copyright)
- [Nominatim usage policy](https://operations.osmfoundation.org/policies/nominatim/):
  the one-time 15-settlement extraction was cached, identified and performed
  sequentially at less than one request/second; the app makes no geocoder requests.
- [Broadcastify developer access](https://support.broadcastify.com/hc/en-us/articles/204740425-Live-Audio-Feed-Catalog-API)
- [OpenMHz API and content terms](https://openmhz.com/terms)
