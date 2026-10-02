## 1. Engine foundation

- [x] 1.1 Confirm GPUI compatibility with the project toolchain, pin an exact published version or immutable Git revision, and add a minimal native app entry point; verify `cargo check` succeeds with the pinned dependency.
- [x] 1.2 Reorganize or wrap the existing audio, detector, MIDI, onset, calibration, feature, template, and note modules under a GPUI-independent engine boundary; verify existing engine and template tests still pass.
- [x] 1.3 Define `PssEngine`, command/event types, and a worker-owned resource model for capture, detector, and MIDI state; verify command delivery and worker shutdown with controllable test boundaries.
- [x] 1.4 Refactor detector processing to return typed note decisions, YIN/spectral results, latency, confidence/margin, and ranked matches instead of UI-relevant stdout output; verify existing detector regression tests and new result tests pass.
- [x] 1.5 Implement worker handling for start, stop, mode changes, device changes, and shutdown, including active-note release and bounded command responsiveness during capture; verify lifecycle and mode-transition tests pass.
- [x] 1.6 Add an engine-level audio-device provider for enumeration, selection, and recoverable open failures; verify saved-device/default selection and unavailable-device behavior with provider tests.
- [x] 1.7 Add validated serde settings at `~/.config/pss2midi/config.json`, keeping templates separate and saving outside audio processing; verify round-trip, safe-default, invalid-config, and atomic-save behavior.
- [x] 1.8 Extract calibration into an engine session with quality checks, retry/cancel, progress events, and a separate template persistence task; verify rejected samples do not advance and completed templates save/load successfully.

## 2. GPUI app and views

- [x] 2.1 Implement the UI `AppState` and event reducer, including a bounded recent-event history and coalesced level updates; verify note, detector, lifecycle, calibration, and error events update state correctly.
- [x] 2.2 Build the GPUI app shell with centralized theme, top bar, Live/Calibration/Settings navigation, and target window sizing; verify all pages are reachable and the layout remains usable at approximately 850×600.
- [x] 2.3 Implement reusable status, meter, detector, match-row, event-log, and 37-key piano components; verify MIDI 36–72 mapping, white/black key order, and C2/C3/C4/C5 labels with unit tests.
- [x] 2.4 Build the Live view from actual engine state, including dominant note, latency, levels, YIN/spectral results, disagreement, top three matches, recent events, and keyboard highlights; verify displayed values update from test engine events.
- [x] 2.5 Add Start/Stop, audio-device selection/retry, detector-mode controls, and MIDI output status to the UI; verify controls issue engine commands and reflect success and recoverable errors.
- [x] 2.6 Build the Calibration view with requested note/sample progress, quality feedback, retry/cancel actions, and completed/current/remaining keyboard states; verify UI state across accepted, rejected, retried, and canceled samples.
- [x] 2.7 Build the Settings view for audio, detector, spectral, MIDI status, and collapsed advanced controls; verify edits are reflected in `AppConfig` and survive serialization/reload.

## 3. Startup and workflow integration

- [ ] 3.1 Wire startup to load settings, enumerate audio devices, load templates when present, initialize MIDI, open Live, and honor saved auto-start behavior; verify first launch and missing-template startup remain usable.
- [ ] 3.2 Connect calibration completion and Start Playing to template save/reload, Spectral mode selection, Live navigation, and engine start when stopped; verify a completed calibration can drive spectral detection.
- [ ] 3.3 Connect GPUI close handling to engine shutdown and worker join, ensuring active MIDI notes are released and audio resources are dropped; verify shutdown with an active note and during capture.
- [ ] 3.4 Replace CLI-first binary startup with direct GUI launch and remove unused CLI-only code/dependencies; verify `cargo run --release` opens the desktop app without CLI arguments.

## 4. Verification

- [ ] 4.1 Add or complete tests for note-name/key mapping, event reduction, detector modes, settings serialization, and calibration progress; verify `cargo fmt --check` and `cargo test` pass.
- [ ] 4.2 Build the release binary and validate on the target Gentoo/Linux setup that audio input selection, YIN/Spectral/Compare modes, MIDI output to Neothesia, calibration save/load, error display, and clean shutdown work end to end.
