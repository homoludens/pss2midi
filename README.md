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

The app opens on Live with the engine stopped by default. Connect the keyboard
audio to a system input, choose that input, and press **Start**. Select
`PSS-F30 Audio MIDI` as the MIDI input in Neothesia or another MIDI application.
The YIN detector works without setup; Spectral requires calibration.

See the [user guide](docs/USAGE.md) for the full walkthrough, calibration
instructions, and an explanation of every setting and control. Settings are
stored separately from spectral samples in
`~/.config/pss2midi/config.json` and
`~/.config/pss2midi/pss-f30-templates.json`, respectively.

## License

Licensed under the [GNU General Public License v3.0](LICENSE).
