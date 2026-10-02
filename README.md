# pss2midi

`pss2midi` converts mono audio from a Yamaha PSS-F30 into MIDI Note On and
Note Off events. It captures audio through ALSA/PipeWire, detects attacks with
aubio, and creates the `PSS-F30 Audio MIDI` virtual output port. The original
aubio/YIN pitch detector remains the default. A calibrated spectral template
detector and a side-by-side comparison mode are also available.

## Requirements

- Linux with ALSA and PipeWire
- An audio source connected to the keyboard output
- ALSA development libraries and `aubio` development files
- Rust stable toolchain

## Build

```bash
cargo build --release
```

## Run

```bash
./target/release/pss2midi \
  --device pipewire \
  --hop 128 \
  --pitch-buffer 2048 \
  --onset-buffer 1024 \
  --silence-db -45 \
  --attack-ignore-ms 10 \
  --decision-window-ms 20 \
  --release-ms 30 \
  --retrigger-ms 90 \
  --initial-stable 10 \
  --debug
```

Select `PSS-F30 Audio MIDI` as the MIDI input in Neothesia or another MIDI
application.

The default is the existing YIN detector, so old commands remain valid. The
explicit equivalent is `--detector yin`.

## Calibrate spectral templates

Calibration records five spectral examples for each key from MIDI 36 (C2)
through MIDI 72 (C5). Connect the keyboard audio, run the command, press Enter
at each prompt to arm the requested key, then play that physical key. The
calibration does not need MIDI input. Press the shown key once per accepted
sample and let its short sound finish before the next prompt.

```bash
./target/release/pss2midi calibrate \
  --device pipewire \
  --samples-per-note 5 \
  --output ~/.config/pss2midi/pss-f30-templates.json
```

The default output path is `~/.config/pss2midi/pss-f30-templates.json`. The
file stores all normalized feature examples plus the sample rate and spectral
settings used to capture them. Runtime rejects a template file whose settings
do not match the selected input settings.

## Detection modes

Use the calibrated spectral classifier as the MIDI authority:

```bash
./target/release/pss2midi --detector spectral \
  --templates ~/.config/pss2midi/pss-f30-templates.json \
  --device pipewire --debug
```

Compare YIN and spectral decisions while spectral remains authoritative:

```bash
./target/release/pss2midi --detector compare \
  --templates ~/.config/pss2midi/pss-f30-templates.json \
  --device pipewire --debug
```

Compare mode prints both decisions, spectral score and margin, agreement
counters, and YIN-to-spectral disagreement counts on Ctrl+C. Spectral capture
uses sample-indexed buffering after each onset; it does not sleep or block the
audio loop. Once a note is active, a classification can change it only after
a new onset. Same-note retrigger and silence release use the existing MIDI
state machine.

Spectral capture options are `--spectral-delay-ms` (default 8),
`--spectral-window-ms` (default 30), and `--fft-size` (default 2048). The
acceptance thresholds `--spectral-min-score` (default 0.75) and
`--spectral-min-margin` (default 0.03) can be tuned for the calibrated
keyboard/input gain.

## Tuning

Start with `--debug`. Each onset shows its pitch votes and selected MIDI note.
`--silence-db` controls release detection. `--initial-stable` is used only
when aubio misses an onset; use `10` to avoid accepting the quiet tail of a
previous note as a new note.

## Project layout

- `src/main.rs`: command startup, capture loop, and shutdown
- `src/config.rs`: command line options
- `src/audio.rs`: ALSA capture configuration
- `src/onset.rs`: shared aubio onset detection
- `src/detector.rs`: YIN, spectral, compare, and note decision state
- `src/features.rs`: sample-indexed buffering and shared FFT feature extraction
- `src/templates.rs`: versioned spectral template format and classification
- `src/calibration.rs`: interactive all-key template calibration
- `src/midi.rs`: virtual MIDI output
- `src/note.rs`: note range, naming, and pitch helpers

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
