# Streaming 1kEE with OBS

OBS captures the native 1kEE window, including the GPU globe, terrain and live
layers. The app's Gruve companion page is a separate simplified map and is not
the native render. No browser source or extra OBS plugin is needed for window
capture.

## Choose what goes on air

- **Full dashboard:** leave the interface as it is and capture the whole window.
- **Clean map:** click **OBS VIEW** in the map toolbar or press **F10** (on a Mac
  keyboard, possibly **Fn-F10**). Press **Escape** or **F10** to restore controls.

Clean view keeps enabled event/camera pips and other map layers. An open **Factal
Brief** stays visible, including briefs opened by **FOLLOW EVENTS**. An open
live-camera window also stays visible. Navigation, live updates, replay and
cinematic movement continue.

The header, layer/event drawers, settings windows, map toolbar, local footer,
hover cards, transport detail windows and loading indicators are hidden. Their
open/closed state and layer choices are preserved when you leave clean view.
The toggle lasts only for this app session and does not change saved settings.

Set your layers, open the brief, and optionally enable **CINEMATIC → FOLLOW
EVENTS** or **MEANDER** before entering clean view. OBS view itself does not
change camera movement or enable additional layers.

## OBS setup on macOS

1. Launch 1kEE and arrange the map and any content windows.
2. In OBS, add a **macOS Screen Capture** source.
3. Set **Method → Window Capture** and choose
   **1kEE | One Thousand Electric Eye**.
4. Disable **Show cursor** if you do not want the pointer in the stream.
5. Fit the source to your OBS canvas. Use a 16:9 window or crop/letterbox it in
   OBS for a 1920×1080 output; clean view uses the current window size and does
   not promise a fixed output resolution or frame rate.
6. Make a short local recording to verify the globe, pips, open brief, and
   transitions before starting your stream.

Allow OBS screen-recording access if macOS requests it. If the window is absent
from the picker, enable **Show fullscreen and hidden windows / applications**.
Keep 1kEE running and avoid minimizing it during capture; capture of an
occluded/minimized window should be checked on your particular OS/OBS version.

This is a visual source. Add your microphone or other audio in OBS as desired.
On Windows use Window Capture; on Linux use the window/screen capture source
available for your display system.

Official references: [macOS Screen Capture](https://obsproject.com/kb/macos-screen-capture-source)
and [OBS sources guide](https://obsproject.com/kb/sources-guide).
