# player_ffplay.rs

## Purpose
Provides non-macOS radio playback through an optional ffplay installation.
A worker owns the subprocess so launch, termination and reaping do not block UI.

## Contracts
- Arguments use `Command` directly, never a shell; only HTTP(S) input and required
  HTTP transport protocols are enabled.
- Stop or channel Drop kills and reaps the child. No standalone player window opens.
- Pause stops the live connection; Resume and volume changes reconnect it.
- Status stays `Connecting` because process existence cannot prove audio output.
  Exit and missing executables are surfaced as errors.
