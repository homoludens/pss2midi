# pss2midi

`pss2midi` converts audio from a Yamaha PSS-F30 into MIDI Note On and Note
Off events. It captures mono PCM audio through ALSA, uses aubio for onset and
pitch detection, then creates the `PSS-F30 Audio MIDI` virtual output port.

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

## Tuning

Start with `--debug`. Each onset shows its pitch votes and selected MIDI note.
`--silence-db` controls release detection. `--initial-stable` is used only
when aubio misses an onset; use `10` to avoid accepting the quiet tail of a
previous note as a new note.

## Project layout

- `src/main.rs`: command startup, capture loop, and shutdown
- `src/config.rs`: command line options
- `src/audio.rs`: ALSA capture configuration
- `src/detector.rs`: onset, pitch, fallback, and note decision state
- `src/midi.rs`: virtual MIDI output
- `src/note.rs`: note range, naming, and pitch helpers

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
