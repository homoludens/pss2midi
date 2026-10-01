Build a small low-latency Rust application called `pss2midi`.

Goal
----
Convert audio from a Yamaha PSS-F30 keyboard into MIDI notes in real time so it can be used as MIDI input in Neothesia.

Current hardware/software setup
-------------------------------
Keyboard:
- Yamaha PSS-F30
- 37 keys
- Range: C2-C5
- MIDI note range: 36-72
- No native MIDI output

Audio path:
PSS-F30 PHONES/OUTPUT
→ analog attenuator
→ AB13X USB headset adapter microphone input
→ Linux PipeWire
→ application

Linux:
- Gentoo
- PipeWire
- ALSA
- aubio 0.4.9 installed system-wide
- AB13X capture source works in pavucontrol
- Neothesia is already installed
- Need a virtual MIDI port visible to Neothesia

Previous Python prototype
-------------------------
The Python version used:
- aubio
- sounddevice
- python-rtmidi
- PipeWire
- 48 kHz audio
- mono input
- virtual MIDI output

The Python version works, but:
- latency is too high
- occasional false notes appear during note attacks
- fast repeated presses of the same key are difficult to detect
- waiting for many stable frames improves reliability but makes latency worse

The previous usable settings were roughly:
- sample rate: 48000
- hop size: 256
- buffer size: 4096
- silence threshold: -45 dBFS
- stable frames: 3
- change-stable frames: 10
- release frames: 6

This produced correct notes most of the time, but latency is too noticeable.

Architecture
------------
Implement:

PSS-F30 audio
→ PipeWire/ALSA capture
→ Rust real-time audio processing
→ aubio onset + pitch detection
→ custom note decision/state machine
→ virtual ALSA MIDI output
→ Neothesia

Preferred Rust stack
--------------------
Use:
- `cpal` for audio capture initially
- `aubio` Rust bindings for aubio
- `midir` for MIDI output
- `anyhow` for errors
- `clap` for CLI
- optionally `ringbuf` or a lock-free channel if needed

Prefer linking against the system-installed aubio via pkg-config rather than building aubio from source.

If Rust aubio bindings become awkward or incompatible:
- use aubio through FFI directly
- do not replace aubio with a large ML model

Important performance rule
--------------------------
Do NOT simply port the Python state machine line-for-line.

The main goal is lower latency.

Design the detector around note attacks.

Suggested strategy
------------------
The PSS-F30 produces misleading harmonics immediately after a key attack.

Example for a physical C3 press:

    64
    40
    48
    48
    48

where 48 is the real C3.

Instead of requiring 10 identical frames, use:

1. detect an onset
2. ignore only the first few milliseconds of the attack
3. collect several pitch estimates over a short decision window
4. reject invalid/out-of-range values
5. choose a robust result using median/mode/weighted voting
6. restrict result to MIDI 36-72
7. emit MIDI as soon as confidence is sufficient

Target parameters to explore:
- sample rate: 48000 Hz
- hop: 128 samples (~2.67 ms)
- pitch buffer: 1024 or 2048 samples
- attack-ignore: ~5-10 ms
- decision window: ~15-25 ms
- target end-to-end audio→MIDI latency: ideally 25-50 ms

Do not delay every note by a long fixed stability window if avoidable.

Pitch detection
---------------
Start with aubio YIN.

Use:
- mono float32 audio
- pitch output as MIDI or Hz
- convert to nearest MIDI note
- reject anything outside MIDI 36-72

Do not rely heavily on aubio `get_confidence()`.
In the Python prototype its values were not useful/reliable.

Use actual pitch stability and agreement across samples instead.

False-note suppression
----------------------
Attack harmonics must not produce real MIDI notes.

The detector should distinguish:
- initial note
- genuine change to another key
- temporary pitch error/harmonic
- repeated press of the same physical key

For a genuine note change:
- do not immediately trust one pitch estimate
- use a short vote/window
- prefer the previous note if the new pitch is unstable
- commit quickly once the new note dominates the decision window

Same-note retrigger
-------------------
This is important.

Example:

    C3 C3 C3 C3

Fast repeated presses may never fall below the silence threshold.

Use aubio onset detection to recognize a fresh attack while the same pitch remains active.

For a repeated same-note attack:
- send NOTE OFF for the current MIDI note
- optionally wait ~1-2 ms
- send NOTE ON again
- apply a small retrigger cooldown such as 50-80 ms
- avoid duplicate retriggers caused by one physical attack

Release detection
-----------------
Do not send NOTE OFF merely because one or more pitch frames are invalid.

Use amplitude/envelope to detect release.

Suggested:
- calculate RMS in dBFS
- silence threshold around -45 dBFS initially
- require a short duration of real silence before NOTE OFF
- perhaps use hysteresis:
  - note-on signal threshold
  - lower note-off threshold

This should avoid false off/on pairs during note decay.

Audio capture
-------------
The physical AB13X device sometimes appears oddly through direct PortAudio/ALSA enumeration.

PipeWire correctly exposes the recording source.

So:
- default to the PipeWire input
- provide `--device` selection
- provide `--list-devices`
- print device name, sample rate, channels

The app should work with:

    pss2midi --device pipewire

or an equivalent device selector.

If CPAL's device naming differs, expose whatever is practical but make selection easy.

MIDI output
-----------
Create a virtual MIDI output/input endpoint visible to Linux apps, named:

    PSS-F30 Audio MIDI

Neothesia must be able to select it as MIDI input.

Send:
- Note On
- Note Off
- channel 1
- fixed velocity initially, e.g. 100

Optionally later derive velocity from onset amplitude, but do not prioritize that now.

CLI
---
Use clap.

Desired commands/options:

    pss2midi --list-devices

    pss2midi \
      --device pipewire \
      --sample-rate 48000 \
      --hop 128 \
      --buffer 2048 \
      --silence-db -45 \
      --attack-ignore-ms 8 \
      --decision-window-ms 20 \
      --release-ms 30 \
      --retrigger-ms 60

Also support:

    --debug

Normal output should be concise:

    ON  C3  48
    OFF C3  48
    ON  D3  50

Debug output should include:
- RMS dBFS
- detected pitch
- nearest MIDI note
- onset detection
- decision window contents
- selected note
- rejection reason
- timing/latency estimate

Avoid rewriting one terminal line during debug.
Print normal timestamped lines so logs can be inspected.

Latency instrumentation
-----------------------
Add simple timing instrumentation.

For each detected onset, measure approximately:
- onset timestamp
- time MIDI NOTE ON is sent

Print in debug mode:

    onset -> MIDI: 31.4 ms

This is important for tuning.

Project structure
-----------------
Keep the project small and readable.

Example:

    pss2midi/
      Cargo.toml
      src/
        main.rs
        audio.rs
        detector.rs
        midi.rs
        cli.rs

Do not over-engineer.

Detector logic should be testable independently from live audio.

Tests
-----
Add unit tests for note-decision logic using synthetic sequences such as:

1. Stable note:

    [48, 48, 48, 48]

=> C3

2. Attack harmonic:

    [64, 40, 48, 48, 48, 48]

=> C3 only
=> no E2/F4 MIDI events

3. Change:

    [48, 48, 48, 50, 50, 50, 50]

=> C3 then D3

4. Bad transient:

    [48, 48, 40, 72, 48, 48]

=> remain C3

5. Same-note retrigger:
- active note 48
- new onset
- pitch remains around 48

=> NOTE OFF 48
=> NOTE ON 48

6. Out-of-range pitch:

    20
    80

=> ignored

7. Silence:
- active C3
- RMS below threshold for release duration

=> NOTE OFF C3

Very important
--------------
Avoid allocations and locks in the real-time audio callback as much as reasonably possible.

The callback should preferably:
- copy/push audio into a preallocated ring buffer
- return quickly

Do heavier aubio processing outside the callback if CPAL timing remains safe.

Do not print from the real-time callback.

Start simple, measure, then optimize.

Deliverables
------------
1. Complete Rust source code
2. Cargo.toml
3. build instructions for Gentoo
4. required system packages
5. run examples
6. explanation of how Neothesia connects to the virtual MIDI port
7. notes about PipeWire/ALSA permissions if needed
8. benchmark/debug instructions for measuring latency

Gentoo considerations
---------------------
System aubio is already installed and works with Python 3.14.

Use pkg-config to find it where possible.

Useful system libraries are likely:
- media-libs/aubio
- media-libs/alsa-lib
- media-video/pipewire
- pkg-config/pkgconf

Do not assume Ubuntu package names.

Success criteria
----------------
Primary test:

Play:

    C3 D3 E3 F3 G3

Expected exactly:

    ON C3
    OFF C3
    ON D3
    OFF D3
    ON E3
    OFF E3
    ON F3
    OFF F3
    ON G3
    OFF G3

No harmonic false notes.

Then play quickly:

    C3 C3 C3 C3

Expected four distinct C3 Note On events.

Then play a simple melody at normal speed in Neothesia.

Prioritize, in this order:

1. correct note
2. no false notes
3. low latency
4. correct same-key retrigger
5. clean architecture

Do not spend time on GUI, configuration files, velocity estimation, chords, polyphony, packaging, or installer yet.

This version is strictly monophonic.
