# geo.rs

Geographic positions and persistent globe/local camera state. `focus_on` is the
ordinary immediate selection behavior; event-follow flights update the camera
without calling it. `stop_motion` clears all inertia and competing idle motion
when event follow takes or releases control. Globe longitude is wrapped on read;
local camera heading is independent of geographic longitude.
