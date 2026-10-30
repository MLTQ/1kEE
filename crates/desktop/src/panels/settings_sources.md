# settings_sources.rs

Render Settings → Data Sources: masked Factal, Windy, 511NY and AISStream keys,
public-directory opt-in/scope/request pace, and saved camera endpoint controls.
Source forms retain their existing model fields and save/poll semantics. Endpoint
work delegates to camera_endpoint_settings; it remains off the rendering thread.

Factal help states the complete severity 2+ past-24-hour live range, one-minute
cooldown after each sync, and replay access to older history.
