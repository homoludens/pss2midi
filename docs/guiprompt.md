Update the existing Rust `pss2midi` project into a GUI-only desktop application using GPUI.

Do NOT preserve the CLI as a first-class interface.
It is okay to remove or greatly reduce the existing clap/CLI layer.

The audio, detector, calibration and MIDI logic should remain modular and independent from GPUI, but the only user-facing interface should now be the desktop GUI.

==================================================
PROJECT CONTEXT
==================================================

Existing app:
- Rust
- Gentoo Linux
- PipeWire / ALSA audio capture
- Yamaha PSS-F30
- 37 keys
- C2-C5
- MIDI notes 36-72
- aubio onset detection
- YIN detector
- spectral-template detector
- compare mode
- virtual MIDI output:
  `PSS-F30 Audio MIDI`
- Neothesia consumes the MIDI output

Detection modes:
- YIN
- Spectral
- Compare

Spectral detector supports calibration of this exact Yamaha keyboard.

==================================================
GOAL
==================================================

Create a polished GPUI desktop application for controlling and visualizing the existing engine.

The GUI must support:

- Start / Stop
- input device selection
- detector mode selection
- live detected-note display
- audio level meter
- latency display
- YIN result
- spectral result
- spectral confidence/margin
- top spectral matches
- recent events
- 37-key keyboard visualization
- spectral calibration wizard
- settings
- MIDI output status

No terminal should be required for normal use.

==================================================
ARCHITECTURE
==================================================

Refactor to something similar to:

src/
  main.rs
  app.rs

  engine/
    mod.rs
    audio.rs
    midi.rs
    onset.rs
    state.rs
    config.rs

    detector/
      mod.rs
      yin.rs
      spectral.rs

    calibration.rs
    templates.rs
    features.rs

  ui/
    mod.rs
    theme.rs
    live.rs
    calibration.rs
    settings.rs

    components/
      mod.rs
      piano.rs
      level_meter.rs
      detector_card.rs
      status_badge.rs
      event_log.rs

The engine MUST NOT depend on GPUI.

Dependency direction:

    engine
       ↑
      GUI

GPUI must only consume engine state/events.

==================================================
ENGINE
==================================================

Create a reusable engine abstraction.

Conceptually:

    pub struct PssEngine {
        ...
    }

Useful operations:

    start()
    stop()

    set_detector_mode(...)
    set_audio_device(...)

    reload_templates(...)

    begin_calibration(...)
    cancel_calibration(...)
    retry_calibration_sample(...)

The audio/detection processing must run outside the GPUI UI thread.

Never process audio inside GPUI rendering callbacks.

==================================================
ENGINE EVENTS
==================================================

Create a lightweight event system.

For example:

    pub enum EngineEvent {
        EngineStarted,
        EngineStopped,

        AudioLevel {
            rms_db: f32,
            peak_db: f32,
        },

        Onset,

        NoteOn {
            midi: u8,
            latency_ms: Option<f32>,
        },

        NoteOff {
            midi: u8,
        },

        Detection {
            selected: Option<u8>,
            yin: Option<YinDetection>,
            spectral: Option<SpectralDetection>,
            latency_ms: Option<f32>,
        },

        CalibrationProgress {
            note: u8,
            sample_index: usize,
            sample_total: usize,
            rms_db: f32,
            peak_db: f32,
        },

        CalibrationNoteComplete {
            note: u8,
        },

        CalibrationComplete {
            template_path: PathBuf,
        },

        Warning(String),
        Error(String),
    }

Do not send raw audio buffers to GPUI.

Do not send every analysis frame.

Only send high-level state updates.

==================================================
GUI TECHNOLOGY
==================================================

Use GPUI.

Pin an exact GPUI version or git revision.

Do not use:
- egui
- iced
- gtk
- slint
- tauri
- web frontend

Use GPUI only.

Use native GPUI layout with:

    div()
    flex()
    child()
    etc.

==================================================
VISUAL DESIGN
==================================================

Use the supplied mockup as the visual reference.

Target style:
- dark desktop utility
- Zed-like restrained styling
- charcoal background
- dark panels
- thin subtle borders
- blue primary accent
- green success/running
- amber warnings/disagreement
- compact typography
- moderate rounding
- no excessive gradients
- no DAW-like clutter

Initial window:

    1000 x 680

Minimum reasonable window size:

    ~850 x 600

==================================================
APP LAYOUT
==================================================

Top bar:

    pss2midi
    Yamaha PSS-F30 Audio to MIDI

                             ● Running   Stop

Left navigation:

    Live
    Calibration
    Settings

Main content changes by page.

==================================================
LIVE PAGE
==================================================

Follow the mockup closely.

Main detected-note card:

    Detected Note

            C3

          MIDI 48

    Latency
    29 ms

    Input Level
    -16 dB
    ████████████████-----

The detected note should be the visually dominant item.

==================================================
SPECTRAL DETECTOR CARD
==================================================

Show:

    Spectral Detector

        C3
        MIDI 48

        Confidence
        0.94

        Margin
        0.16

        Selected

Use progress bars for confidence and margin.

When spectral mode is authoritative, show:

    Selected

as a green badge.

==================================================
YIN DETECTOR CARD
==================================================

Show:

    YIN Detector

        G3
        MIDI 55

If disagreement:

    ⚠ Disagrees with spectral

Use amber warning styling.

If detectors agree, show subtle success state instead.

==================================================
TOP SPECTRAL MATCHES
==================================================

Show the best 3 classifications:

    Top Spectral Matches

    1. C3 (48)    ███████████  0.94
    2. G3 (55)    █████████    0.78
    3. E3 (52)    ██████       0.62

This data must come from the actual detector.

==================================================
EVENT LOG
==================================================

Show a small recent-event panel:

    Recent Events

    10:42:14.231   onset
    10:42:14.261   C3 spectral=.94 YIN=G3 30ms
    10:42:14.422   off C3

Maintain approximately:
    100 events internally

Display:
    6-12 recent events

Allow:

    [x] Show log

Do not implement a complicated logging system.

==================================================
PIANO KEYBOARD
==================================================

Implement a custom 37-key keyboard.

Range:

    C2-C5

MIDI:

    36-72

Correctly position white and black keys.

Show labels:

    C2
    C3
    C4
    C5

State visualization:

Current selected note:
    blue filled highlight

YIN disagreement:
    amber outline or marker

Calibration current key:
    blue

Calibration completed:
    subtle success marker

Idle:
    normal piano appearance

The piano is display-only initially.

It does not need mouse interaction.

==================================================
BOTTOM STATUS AREA
==================================================

As in the mockup, include compact panels:

AUDIO INPUT

    AB13X Headset Adapter (PipeWire)

    Sample rate
    48 kHz

    Channels
    Mono

    Input
    -16 dB

    Peak
    -5.8 dB


MIDI OUTPUT

    PSS-F30 Audio MIDI

    ● Connected

Optionally include a MIDI test button:

    Test Note

If implemented, test note must not interfere with the detector state.


DETECTOR MODE

    ○ YIN

    ● Spectral
      Calibrated template matching

    ○ Compare
      Show both detectors

==================================================
DETECTOR MODE
==================================================

Support:

    YIN
    Spectral
    Compare

YIN:
- current YIN detector drives MIDI

Spectral:
- spectral detector drives MIDI

Compare:
- run both
- spectral drives MIDI initially
- display disagreements

Changing mode through GUI should preferably work while running.

If that is unsafe, restart the engine automatically.

Do not require restarting the whole application.

==================================================
START / STOP
==================================================

Top-right Start/Stop controls the actual engine.

Running:

    ● Running
    Stop

Stopped:

    ○ Stopped
    Start

Stop must:
- stop audio capture
- send NOTE OFF for active note
- stop detector activity
- cleanly release audio resources

Start must:
- reopen audio input
- restore detector
- ensure MIDI output exists

Closing the app must shut down everything cleanly.

No hanging MIDI note.

==================================================
CALIBRATION PAGE
==================================================

Create a guided calibration wizard.

Do not expose calibration primarily through terminal.

Calibration should be easy enough to use without understanding DSP.

Main calibration view:

    Calibration

                C3
              MIDI 48

           Press this key

               3 / 5

           ● ● ● ○ ○

    Input Level
    ███████████████-----
    -17 dB

    Peak
    -5.6 dB

    [ Retry ]
    [ Cancel ]

Below:
show the full 37-key piano.

Completed notes:
    marked complete

Current note:
    highlighted

Remaining:
    neutral

==================================================
CALIBRATION WORKFLOW
==================================================

The spectral detector needs templates for every key.

Range:

    MIDI 36-72
    C2-C5

Default:

    5 samples per note

Workflow:

1. user opens Calibration
2. click:
       Start Calibration
3. show:
       Press C2
4. detect onset
5. capture feature
6. show:
       sample 1 / 5
7. repeat until 5 samples
8. mark C2 complete
9. continue C#2
10. continue through C5
11. save templates
12. show completion

Do NOT infer which key is being pressed during calibration.

The current requested key is known.

User is expected to press that key.

==================================================
CALIBRATION QUALITY
==================================================

Show:

    RMS
    peak

Reject or warn for:

- input too quiet
- clipping
- invalid capture

Example:

    Sample accepted

or:

    Input too quiet
    Please press again

Allow:

    Retry sample

The UI should not advance on a rejected sample.

==================================================
CALIBRATION COMPLETION
==================================================

Show:

    Calibration complete

    37 notes
    185 samples

    Templates saved to:

    ~/.config/pss2midi/pss-f30-templates.json

    [ Start Playing ]

Clicking Start Playing:
- switch to Live
- switch detector to Spectral
- load new templates
- start engine if needed

==================================================
SETTINGS PAGE
==================================================

Keep Settings simple.

AUDIO

    Input Device

    [ AB13X Headset Adapter            ▼ ]

    Sample Rate
    [ 48000 ]

    Status
    ● Available


DETECTOR

    ○ YIN
    ● Spectral
    ○ Compare


SPECTRAL

    Template file

    ~/.config/pss2midi/pss-f30-templates.json

    [ Recalibrate ]

    Delay
    8 ms

    Window
    30 ms

    Minimum score
    0.75

    Minimum margin
    0.03


MIDI

    PSS-F30 Audio MIDI

    ● Connected


ADVANCED

Collapsed by default.

Put advanced options there:

- FFT size
- silence dB
- release ms
- retrigger ms
- onset threshold
- YIN parameters
- spectral tuning

Normal users should not need Advanced.

==================================================
DEVICE SELECTION
==================================================

Enumerate available capture devices.

Provide dropdown selection.

Default to:
- previously saved device
- otherwise PipeWire default input

Show device errors in GUI.

If device disappears:

    Audio device unavailable

    [ Retry ]

Do not crash.

==================================================
APP STATE
==================================================

Create a GUI state model.

Conceptually:

    pub struct AppState {
        pub page: Page,
        pub running: bool,

        pub current_note: Option<u8>,
        pub latency_ms: Option<f32>,

        pub rms_db: f32,
        pub peak_db: f32,

        pub detector_mode: DetectorMode,

        pub yin_result: Option<YinDetection>,
        pub spectral_result: Option<SpectralDetection>,

        pub audio_device: Option<String>,
        pub midi_connected: bool,

        pub calibration: CalibrationUiState,

        pub recent_events: VecDeque<UiEvent>,
    }

Keep actual DSP objects out of GUI state.

==================================================
THREADING
==================================================

Critical:

GPUI must never block audio processing.

Architecture:

    ALSA/PipeWire capture
            ↓
       engine thread
            ↓
    detector / MIDI
            ↓
       EngineEvent
            ↓
          channel
            ↓
          GPUI

The audio callback/thread must NOT:
- render
- call GPUI
- write JSON
- print logs
- block on UI mutex
- perform expensive allocations unnecessarily

The GUI consumes events asynchronously.

==================================================
UI UPDATE RATE
==================================================

Do not redraw for every audio hop.

For level meter:

    ~20-30 FPS

is sufficient.

Note events should update immediately.

Spectral/YIN detector result updates should update when decisions occur.

==================================================
CONFIGURATION
==================================================

Persist GUI settings.

Use:

    ~/.config/pss2midi/config.json

Persist things such as:

- audio device
- detector mode
- template file path
- spectral thresholds
- sample rate
- advanced settings
- window size if easy

Templates remain separate:

    ~/.config/pss2midi/pss-f30-templates.json

If config does not exist:
- use safe defaults

If template file does not exist:
- YIN must still work
- Spectral page should show:

      Not calibrated

      [ Calibrate ]

==================================================
STARTUP
==================================================

On launch:

1. load config
2. initialize GPUI
3. enumerate audio devices
4. load spectral templates if available
5. initialize virtual MIDI output
6. open Live page
7. start engine automatically if previous config says so, otherwise show Start

Do not block app startup if spectral templates are missing.

==================================================
ERROR HANDLING
==================================================

Show in-app errors.

Examples:

    Could not open AB13X input

    Template file invalid

    MIDI output creation failed

Use small error banners/cards.

Avoid panicking for recoverable runtime errors.

==================================================
GPUI COMPONENTS
==================================================

Create reusable components where useful:

    PianoKeyboard
    LevelMeter
    DetectorCard
    StatusBadge
    SpectralMatchRow
    EventLog
    NavItem
    SettingsRow

Do not over-componentize every `div()`.

==================================================
THEME
==================================================

Create centralized theme constants.

Example categories:

    background
    panel
    panel_hover
    border
    text_primary
    text_secondary
    accent
    success
    warning
    danger

Do not scatter raw color literals throughout all files.

==================================================
PERFORMANCE
==================================================

The GUI must not increase detector latency significantly.

The engine should behave essentially the same whether the UI is actively redrawing or not.

Avoid:
- sending spectral vectors to UI
- frequent heap allocation
- per-hop UI notifications
- unnecessary locks

==================================================
TESTS
==================================================

Keep all existing engine/detector tests.

Add tests for:

- app-state reducer / event handling
- MIDI note -> note-name mapping
- MIDI note -> piano-key mapping
- 36-72 keyboard range
- settings serialization
- detector-mode changes
- calibration progress state

No screenshot testing required.

==================================================
DEPENDENCIES
==================================================

Keep existing dependencies where needed:

- alsa
- aubio
- midir
- anyhow
- ctrlc if still useful
- rustfft
- serde
- serde_json

Add:

- gpui

Remove clap if it is no longer needed.

Remove CLI-only dependencies/code if unused.

Do not add another GUI framework.

==================================================
IMPLEMENTATION ORDER
==================================================

Do not just provide a plan.

Actually implement it.

Work in this order:

1. inspect the current project
2. preserve working audio/detection/MIDI code
3. remove unnecessary CLI coupling
4. create clean engine API
5. create EngineEvent channel
6. build GPUI app shell
7. implement theme
8. implement sidebar/top bar
9. implement Live page
10. wire real engine events
11. implement PianoKeyboard
12. implement detector cards
13. implement event log
14. implement Start/Stop
15. implement detector mode switching
16. implement Calibration page
17. connect actual calibration system
18. implement Settings page
19. implement persistent config
20. handle runtime errors
21. build release
22. fix all compiler errors
23. run tests
24. provide final usage instructions

==================================================
SUCCESS CRITERIA
==================================================

Running:

    cargo run --release

or:

    ./target/release/pss2midi

must directly open the GUI.

No CLI command should be required.

The GUI should:

- detect the AB13X input
- start/stop capture
- show current audio level
- show detected notes
- create `PSS-F30 Audio MIDI`
- send MIDI to Neothesia
- switch between YIN/Spectral/Compare
- show detector disagreement
- show latency
- show spectral top matches
- calibrate all 37 keys
- save/load templates
- work without terminal interaction

Closing the app must shut down audio and MIDI cleanly.

Most importantly:

KEEP THE AUDIO/DETECTION ENGINE SEPARATE FROM GPUI.

The application is GUI-only from the user's perspective, but DSP code must remain modular, testable and independent of the UI.
