# Using `pss2midi`

`pss2midi` listens to the mono audio output of a Yamaha PSS-F30 and sends MIDI
notes through a virtual MIDI port. It supports the existing aubio/YIN detector
and a spectral detector that uses examples recorded from your keyboard.

Commands below assume you are in the project directory.

## Build and inspect available options

Build the release executable:

```bash
cargo build --release
```

Show the run options or calibration options:

```bash
./target/release/pss2midi --help
./target/release/pss2midi calibrate --help
```

If you need to check device names, list ALSA audio devices and MIDI ports:

```bash
arecord -L
aconnect -l
```

The default audio device is `pipewire`, which uses PipeWire's default source.
The audio output from the keyboard must be connected to that source (or to the
ALSA capture device named with `--device`).

## Run with the existing YIN detector

YIN is the default, so this is equivalent to passing `--detector yin`:

```bash
./target/release/pss2midi \
  --device pipewire \
  --detector yin \
  --hop 128 \
  --pitch-buffer 2048 \
  --onset-buffer 1024 \
  --silence-db=-45 \
  --attack-ignore-ms 10 \
  --decision-window-ms 20 \
  --release-ms 30 \
  --retrigger-ms 90 \
  --initial-stable 10 \
  --debug
```

The program creates the `PSS-F30 Audio MIDI` output port. Select that port as
the MIDI input in Neothesia or your other MIDI application. Keep this terminal
running while playing; press Ctrl+C to stop it.

## Calibrate spectral templates

Calibration records five examples for each physical key from C2 (MIDI 36)
through C5 (MIDI 72). It does not need MIDI input, but the keyboard's audio
must reach the selected audio device.

```bash
./target/release/pss2midi calibrate \
  --device pipewire \
  --samples-per-note 5 \
  --output ~/.config/pss2midi/pss-f30-templates.json
```

For each prompt, press Enter to arm the requested key, then play that physical
key. The program waits for an onset, captures a short audio sample, and reports
whether it was accepted. Play the displayed key for every sample; calibration
does not identify the pitch automatically. Samples that are too quiet are
rejected. A near-clipping peak produces a warning. The template file is saved
after all 37 keys are complete.

The default template path is
`~/.config/pss2midi/pss-f30-templates.json`, so `--output` can be omitted if
you want to use that location.

## Run with the spectral detector

After calibration, start spectral mode and select the template file:

```bash
./target/release/pss2midi \
  --device pipewire \
  --detector spectral \
  --templates ~/.config/pss2midi/pss-f30-templates.json \
  --debug
```

The spectral detector classifies each new onset against the saved examples. It
does not switch an active note based on later pitch estimates without a new
onset. The existing note state machine handles note-off, silence release, and
same-note retriggers.

Calibration and runtime must use the same sample rate and spectral feature
settings. Defaults are 48 kHz, an 8 ms delay after onset, a 30 ms feature
window, and a 2048 point FFT. If you change any spectral capture setting while
calibrating, pass the same value when running. For example, to use a 4096 point
FFT, add `--fft-size 4096` to both commands.

## Compare YIN and spectral decisions

Compare mode runs both detectors on each onset. Spectral remains authoritative
for MIDI output, so this does not send duplicate notes:

```bash
./target/release/pss2midi \
  --device pipewire \
  --detector compare \
  --templates ~/.config/pss2midi/pss-f30-templates.json \
  --debug
```

Debug output shows the YIN candidate, spectral candidate and score/margin,
whether the decisions agree, and the MIDI result. On Ctrl+C, compare mode prints
agreement counts and observed YIN-to-spectral disagreements.

## Useful tuning options

| Option | What it changes |
| --- | --- |
| `--device` | ALSA capture source; defaults to `pipewire` |
| `--silence-db=-45` | Level below which audio is treated as silence |
| `--release-ms 30` | Silence duration required before MIDI note-off |
| `--retrigger-ms 90` | Minimum interval between same-note attacks |
| `--initial-stable 10` | Stable YIN frames needed for fallback acquisition if an onset is missed |
| `--spectral-delay-ms 8` | Audio skipped after onset before a spectral sample |
| `--spectral-window-ms 30` | Duration of the spectral sample |
| `--fft-size 2048` | FFT size; must be a power of two and at least the sample-window length |
| `--spectral-min-score 0.75` | Minimum spectral similarity required for acceptance |
| `--spectral-min-margin 0.03` | Required gap between the best and second-best template scores |

Use `--debug` to see onset, pitch-vote, and classification details. If
spectral results are rejected, inspect their score and margin before changing
the thresholds. If YIN fallback acquires notes from the quiet tail of a sound,
increase `--initial-stable`; fallback is used only when an onset is missed.

For a negative numeric option, the `--option=value` form (for example
`--silence-db=-45`) avoids shell or argument-parsing ambiguity.
