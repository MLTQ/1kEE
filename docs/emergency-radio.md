# Nearby emergency radio

Select an event and click **Nearby radio** in its Brief or focused Event Queue
card. The floating window searches Radio Browser's open directory for police,
fire, EMS, public safety, aviation, marine and weather radio. Filter by type,
name/place and a 25–500 km radius. Press **Play** on a feed; Pause, Resume, Stop
and Volume control the single player. Closing the window stops audio.

The radio window stays pinned to the searched event while Follow Events moves
on. Use **Search selected event** to change areas explicitly. That stops the
previous feed. Nothing plays automatically and there is no startup radio query.

## Coverage

The directory is community-maintained and its emergency/dispatch coverage is
sparse. Some entries are public advisory radio rather than dispatch. Published
station coordinates are approximate and do not establish radio reception,
agency jurisdiction or a connection to the selected incident. Entries without
coordinates are excluded from nearby results. **Browse all open feeds** includes
unmapped and distant entries and labels their distance/status honestly.

Searches fetch a bounded category subset, deduplicate UUIDs and stream URLs,
and cache successful results for 15 minutes. Failed/full subqueries display an
incomplete-results message. Refresh is explicit; failures preserve older results.
Directory listing availability is not a guarantee that an audio feed is online.

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
- [Broadcastify developer access](https://support.broadcastify.com/hc/en-us/articles/204740425-Live-Audio-Feed-Catalog-API)
- [OpenMHz API and content terms](https://openmhz.com/terms)
