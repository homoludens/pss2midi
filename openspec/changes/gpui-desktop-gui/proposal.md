## Why

The current command-line interface makes it difficult to control audio capture, understand detector results, and complete spectral calibration without working in a terminal. A native GPUI application will make the Yamaha PSS-F30 audio-to-MIDI workflow accessible through one cohesive interface while keeping audio and DSP logic independent of the UI.

## What Changes

- Replace the CLI-first user experience with a GPUI desktop application that opens directly into the Live page; trim CLI-only functionality and dependencies where they are no longer needed.
- Add Live, Calibration, and Settings pages following the supplied dark desktop mockup.
- Expose engine controls for starting and stopping capture, selecting the audio device and detector mode, and showing MIDI output and runtime status.
- Display real detection results, audio levels, latency, detector agreement, spectral matches, recent events, and the 37-key C2–C5 keyboard visualization.
- Provide a guided spectral calibration flow with per-sample quality feedback, retry/cancel controls, and template saving.
- Persist audio, detector, spectral, and advanced settings; present recoverable device, template, and MIDI errors in the GUI.
- Keep audio capture, onset detection, YIN and spectral detection, calibration, and MIDI processing modular and independent of GPUI. Deliver engine updates asynchronously as high-level events, and shut down audio and active MIDI notes cleanly on stop or application close.

## Capabilities

### New Capabilities

- `live-monitoring`: Observe live note detection, detector results, levels, latency, recent events, and keyboard activity.
- `audio-midi-control`: Control engine lifecycle, audio input and detector selection, and view MIDI output and runtime status.
- `spectral-calibration`: Guide calibration across all 37 keys, report sample quality and progress, and save templates.
- `application-settings`: Configure and persist audio, detector, spectral, and advanced application settings.

### Modified Capabilities

None. The project currently has no existing OpenSpec capabilities.

## Impact

Changes affect the Rust application entry point and `Cargo.toml`, introducing GPUI while retaining the existing ALSA, aubio, MIDI, FFT, and serialization dependencies where needed. Existing audio, detector, calibration, template, and MIDI modules will be reorganized behind a GPUI-independent engine API; new UI and app-state modules will consume its asynchronous events. The application will use `~/.config/pss2midi/config.json` for settings and keep spectral templates at `~/.config/pss2midi/pss-f30-templates.json`.
