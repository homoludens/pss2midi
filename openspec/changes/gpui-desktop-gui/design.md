## Context

See `proposal.md` for motivation and the four capability specs for externally visible behavior. The current `main.rs` owns CLI parsing, blocking ALSA frame reads, detector processing, and MIDI output in one synchronous run loop. `Detector` currently sends MIDI directly and emits diagnostic text, while calibration prompts and saves templates through terminal I/O. Existing DSP, note mapping, capture, and template code is in top-level `src` modules; GPUI is not yet a dependency.

## Goals / Non-Goals

**Goals:**
- Give the UI an asynchronous, testable engine boundary that does not expose audio buffers or GPUI types to DSP code.
- Preserve the established YIN, spectral, onset, calibration, and MIDI behavior while making decisions and status available as typed engine events.
- Make start/stop, mode changes, calibration, and shutdown deterministic despite the capture loop owning audio and MIDI resources.
- Keep the configuration and spectral template formats separate, with settings and template writes outside real-time audio processing.

**Non-Goals:**
- Changing the note-detection algorithms or introducing a new audio backend as part of the GUI refactor.
- Supporting multiple keyboards or MIDI-routing profiles.
- Making the piano visualization interactive or building a general-purpose logging system.
- Supporting another GUI framework or a web frontend.

## Decisions

### Keep the engine as the owner of DSP and device resources

Organize the crate around `engine`, `ui`, and the GPUI application entry point. Move or wrap the existing audio, detector, MIDI, onset, configuration, calibration, feature, and template modules under `engine` without adding GPUI dependencies to them. Expose a `PssEngine` controller for lifecycle and configuration commands; keep the concrete `PCM`, `Detector`, `Midi`, and calibration-session values private to the engine.

One dedicated worker owns the active capture stream, detector state, MIDI connection, and any calibration session. The UI sends commands such as Start, Stop, SetMode, SetAudioDevice, BeginCalibration, RetrySample, CancelCalibration, ReloadTemplates, and Shutdown through a channel. The worker checks for commands between short capture periods; use ALSA polling or equivalent bounded waiting so device errors and shutdown cannot leave the UI waiting on a blocking read. Shutdown releases an active note before dropping MIDI and audio resources.

Alternative considered: share detector and device objects behind a mutex for direct UI calls. This would couple rendering and control to engine locks and risks stalling audio processing, so device and DSP ownership remains on the worker.

### Convert detector side effects into engine outcomes

Refactor `Detector::process` to produce typed decision updates instead of writing UI-relevant diagnostics to stdout or directly owning the MIDI connection. Updates include onset, note-on/off decisions, selected note, YIN and spectral results, confidence/margin and ranked spectral matches, latency, and level data. The worker applies note decisions to its MIDI connection and publishes corresponding high-level `EngineEvent`s. Existing detector voting and classification rules remain in the engine.

The engine-to-UI channel carries decisions and state changes, never raw audio or per-hop spectral vectors. The GPUI app reduces events into a plain `AppState` containing page, running state, current results, calibration progress, and a bounded recent-event queue. Coalesce audio-level updates to approximately 20–30 Hz; deliver note, error, lifecycle, and calibration events as soon as they occur. The UI's event subscription updates GPUI state asynchronously and rendering only reads that state.

Alternative considered: have the UI poll engine state or request detector objects. Event delivery avoids polling stale state and prevents the UI from entering the processing path.

### Serialize mode and calibration transitions on the worker

Treat commands as state transitions handled by the resource-owning worker. For a mode or device change, first release any active MIDI note and end the old detector/capture resources as needed, then construct the new configuration and report success or a recoverable error. The UI remains alive throughout; it reflects the engine's resulting state rather than assuming a command succeeded.

Calibration uses a `CalibrationSession` built from the existing onset, feature extraction, and template code. It consumes captures from the engine worker, tracks the known requested MIDI note and sample index, and emits progress/quality events without trying to identify the played pitch. On completion, pass extracted calibration data to a separate persistence worker to validate and write the template JSON; do not perform filesystem writes on the capture path. Report completion only after the save succeeds. Start Playing then issues a template reload, mode change to Spectral, and start command as needed.

Alternative considered: keep the terminal calibration routine and invoke it as a subprocess. That retains terminal prompts and splits resource/error handling across processes, so calibration becomes an engine session instead.

### Separate persisted settings from templates

Use a serde-backed `AppConfig` for GUI settings at `~/.config/pss2midi/config.json`; retain the existing template JSON at `~/.config/pss2midi/pss-f30-templates.json`. Load defaults when settings are absent, validate configuration before applying it, and save changes from a non-audio task. Write configuration through a temporary file and rename it into place so an interrupted write does not replace a valid file with a partial one. Keep templates readable by the current classifier; settings never embed template samples.

Represent audio device discovery and opening behind an engine-level provider. Initially use the existing ALSA/PipeWire capture path and device identifiers, with the saved device preferred and `pipewire` as the fallback. This keeps backend-specific enumeration out of the UI and avoids adding a second capture stack.

Alternative considered: store UI preferences in the template file or add a database. Separate JSON files preserve the current template contract and keep the small set of preferences simple to inspect and recover.

### Build the interface with native GPUI and a centralized theme

Use GPUI native layout and widgets for the app shell, top bar, navigation, Live, Calibration, and Settings pages. Keep reusable components focused on visible concepts such as the 37-key piano, detector cards, level meter, status badge, match rows, and event log. Store color, border, spacing, and typography values in a shared theme module rather than scattering literals through page code. Target the supplied 1000×680 mockup and maintain a usable layout down to approximately 850×600.

Pin GPUI to an exact compatible published version or immutable Git revision in `Cargo.toml`; do not use a floating branch. Confirm the pin against the project's Rust toolchain and Gentoo/Linux system dependencies when adding it. GPUI is the only UI framework.

Alternative considered: use a webview or another Rust GUI toolkit. These are excluded by the product constraints and would create a second rendering model rather than the requested native GPUI interface.

### Test the engine boundary and UI state without screenshot tests

Keep existing audio/detector/template tests and add focused unit tests for note-name and MIDI-key mapping, the 36–72 keyboard range, event-to-`AppState` reduction, detector-mode transitions, settings serialization, and calibration progress. Test engine lifecycle transitions with controllable/fake boundaries where practical; retain the real ALSA/MIDI integration as runtime verification on the target Linux system. No screenshot test harness is required.

## Risks / Trade-offs

- **GPUI's pinned revision or system libraries may not build on the target Gentoo installation** → Verify the exact dependency pin and native build prerequisites early, and keep the pin immutable once selected.
- **An ALSA read or device failure could delay Stop or mode changes** → Bound worker waits, check control commands between capture periods, and route recoverable capture errors to engine events.
- **Mode changes, calibration, and MIDI output share stateful resources** → Serialize transitions through the worker; release active notes before replacing detector or device state and report command failures explicitly.
- **Frequent meter updates could increase UI work or detector latency** → Coalesce level events and keep all rendering and GPUI notification work outside the audio-processing path.
- **Template/config writes could stall capture or leave partial data** → Perform persistence on a separate task, use atomic replacement for configuration, and only report successful calibration after template save completes.
- **Removing CLI behavior could affect existing terminal workflows** → Preserve the engine and template formats, document GUI launch as the supported entry point, and keep source-level module tests independent of GPUI.

## Migration Plan

1. Extract current DSP and device code behind the engine API while retaining the existing template JSON format and detector tests.
2. Add the GPUI app, asynchronous event/state bridge, and pages; switch the binary entry point from CLI dispatch to GUI startup and trim CLI-only dependencies once unused.
3. Load existing templates unchanged. Create the settings file with safe defaults on first save; no settings-file migration is needed because the GUI configuration is new.
4. Verify debug and release builds and the existing/new tests on Gentoo/Linux, then verify audio selection, detector modes, MIDI note release, calibration save/load, and application shutdown against the real devices.

Rollback is a binary/source rollback: the pre-GUI executable and CLI code can be restored without changing existing template files. The new settings file is independent and may be left in place or removed; the old CLI does not depend on it.
