# pss2midi

`pss2midi` is a native GPUI desktop application that converts mono audio from a
Yamaha PSS-F30 into MIDI Note On and Note Off events. It captures audio through
ALSA/PipeWire, detects attacks with aubio, and creates the `PSS-F30 Audio MIDI`
virtual output port. The Live page supports YIN, Spectral, and Compare detector
modes; Calibration guides the user through creating spectral templates, and
Settings stores audio and detector preferences.

## Requirements

- Linux with ALSA and PipeWire
- An audio source connected to the keyboard output
- ALSA, aubio, and GPUI native development dependencies
- Rust stable toolchain

## Build and run

```bash
cargo build --release
cargo run --release
```

The application opens directly into the Live page and uses safe settings when
no configuration exists. Select `PSS-F30 Audio MIDI` as the MIDI input in
Neothesia or another MIDI application.

## Calibration

Open the Calibration page to record five accepted spectral examples for each
key from MIDI 36 (C2) through MIDI 72 (C5). Follow the requested note and
sample progress, and retry captures reported as quiet, clipped, or invalid.
Completed templates are saved to
`~/.config/pss2midi/pss-f30-templates.json`. The completion view's **Start
Playing** action loads the saved templates, selects Spectral mode, and starts
the engine if it is stopped.

## Configuration

Settings are stored separately from spectral samples in
`~/.config/pss2midi/config.json`. The Settings page includes audio input,
sample rate, detector mode, spectral thresholds and timing, plus advanced DSP
controls. A missing or invalid template does not prevent YIN operation or
application startup.

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
