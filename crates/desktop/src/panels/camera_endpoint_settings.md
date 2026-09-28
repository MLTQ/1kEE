# camera_endpoint_settings.rs

## Purpose
Render saved camera-directory controls inside Settings → APIs → Project Eyes On.
Keep filesystem and network work off the egui thread.

## Components
- `render` owns scope-specific UI snapshots and a single cache worker per scope.
- `controls` offers Search again, Check saved endpoints, Remove failed, and a
  filtered virtual list with per-endpoint Forget buttons and check ages.
- `run` persists an action/removal before invalidating the registry poll.

## Contracts
- Search/check require the existing directory opt-in; cache removal does not.
- Successful removals immediately update map/nearby/selected camera state and
  preserve unrelated providers. The next registry poll uses the changed cache.
- Failed disk operations surface an error and leave displayed cameras intact.
- Cache snapshots refresh every two seconds while this settings section is open.
- A forgotten endpoint may return only after an explicit new discovery finds it
  reachable; startup and ordinary Poll Now reuse the saved scope, including empty scopes.

An egui headless test checks the refresh/check/removal controls and saved endpoint
list without starting workers or touching the app cache.
