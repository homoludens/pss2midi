# Using `pss2midi`

`pss2midi` is a Linux desktop app that listens to audio from a Yamaha PSS-F30
and sends detected notes through the virtual MIDI output `PSS-F30 Audio MIDI`.
It has three pages: **Live**, **Calibration**, and **Settings**. The pages scroll
vertically with the mouse wheel or trackpad; you can also drag the accent
scrollbar at the right edge of a page.

## Build and launch

You need Linux with ALSA and PipeWire, the ALSA, aubio, and GPUI native
development dependencies, and a stable Rust toolchain. Connect the keyboard's
audio output to an input available to ALSA/PipeWire.

From the project directory, build and launch the app:

```bash
cargo build --release
cargo run --release
```

This is a desktop GUI application; there are no audio-conversion command-line
options.

## Run a binary release

For the prebuilt Linux x86-64 release, download and extract
`pss2midi-0.1.0-linux-x86_64.tar.gz`, then run:

```bash
cd pss2midi-0.1.0-linux-x86_64
./pss2midi
```

The executable is dynamically linked, so the system needs the runtime libraries
for ALSA, aubio, and its X11 or Wayland desktop backend. Package names vary by
Linux distribution. If launch reports a missing shared library, install the
distribution package that provides that library. The downloadable build is
compiled on Gentoo Linux x86-64; it is not a static binary for every Linux
distribution.

## Basic use

1. **Choose the audio input.** In Live or Settings, open the device selector.
   `PipeWire default` uses the system's default PipeWire source; other listed
   entries select an ALSA capture device. Use **Refresh** to enumerate inputs.
   The badge reports whether the selected input is open. If it is not, check
   the system audio route and press **Retry input**.
2. **Start detection.** The app opens on Live and starts stopped by default.
   Press **Start** in the top bar to begin listening. Use **Stop** to stop
   capture and detection.
3. **Receive MIDI.** In Neothesia or another MIDI application, choose
   `PSS-F30 Audio MIDI` as its MIDI input. The app's MIDI badge reports whether
   its virtual output is connected.
4. **Pick a detector.** YIN works without calibration. To use Spectral, first
   complete the calibration steps below. Compare runs YIN and Spectral together
   for inspection, while Spectral remains the detector used for MIDI output.
5. **Check Live.** The Live page shows the selected note, input level, latency,
   detector results, spectral matches, keyboard activity, and recent events.
   Use **Show log** to hide or show the event list.

The Spectral detector needs a template file that matches the spectral capture
settings. After changing sample rate, FFT size, spectral delay, or spectral
window, restart the app and calibrate again before relying on Spectral results.

## Calibrate spectral templates

1. Open **Calibration** and press **Start Calibration**. Calibration stops live
   detection if it is running, then opens the selected input for capture.
2. Play the displayed key on the keyboard. Calibration proceeds from C2 (MIDI
   36) to C5 (MIDI 72), capturing five accepted examples for each of the 37
   notes. It does not identify which key you played, so play the requested note
   each time.
3. Watch the sample quality and progress. A sample can be rejected as too
   quiet, clipped, or invalid. Adjust the input level or playing and try again;
   **Retry sample** requests another capture for the current note. **Cancel**
   stops the run; completed notes remain marked in the progress display.
4. When calibration completes, the templates are saved to the configured
   template path. Press **Start Playing** to load them, switch to Spectral, and
   return to Live. It starts detection if the engine is stopped.

## Controls and status

| Control or status | What it does |
| --- | --- |
| **Start / Stop** | Starts or stops audio capture and note detection. |
| Audio device selector | Selects a listed ALSA input or the PipeWire default source. Its badge shows `OPEN`, `NOT OPEN`, or `ERROR`. |
| **Refresh** | Requests a new list of audio inputs. |
| **Retry input** | Tries to open the selected input again. |
| **YIN** | Uses aubio/YIN pitch estimates; no template file is needed. |
| **Spectral** | Classifies notes against calibrated templates. |
| **Compare** | Displays both detector results; Spectral remains authoritative for MIDI output. |
| MIDI output badge | Reports `NOT STARTED`, `CONNECTED`, `STOPPED`, or `ERROR` for `PSS-F30 Audio MIDI`. |
| Template badge | Reports whether templates are not calibrated, loaded, or in error. |
| **Recalibrate** | Opens the Calibration page. |
| **Show log** | Shows or hides the recent event list on Live. |
| **Advanced** | Expands or collapses the additional DSP controls in Settings. It starts collapsed. |
| **Retry sample** | Discards the in-progress calibration capture and waits for the same note/sample again. |
| **Cancel** | Cancels the current calibration run. |
| **Start Playing** | Loads the completed templates, selects Spectral, and starts the engine if stopped. |

## Settings reference

Click a value in Settings to edit it. Valid changes are saved automatically to
`~/.config/pss2midi/config.json`. Press **Enter** to finish editing; **Escape**
closes the editor but does not undo values already accepted. Invalid text is
not saved and the page shows an error.

Audio-device selection, detector mode, and template path are sent to the engine
immediately. Numeric processing settings (including sample rate) are saved for
the next app launch, so restart the app after changing them. Recalibrate after
changing a spectral capture setting.

### Audio and detector

| Setting | Default | Meaning |
| --- | --- | --- |
| Audio device (`audio.device`) | `pipewire` | Input source used to capture the keyboard's audio. Choose `PipeWire default` or an enumerated ALSA device in the selector. |
| Detector mode (`detector_mode`) | `yin` | Selects YIN, Spectral, or Compare. Spectral needs valid templates; Compare shows both results and uses Spectral for MIDI output. |
| Sample rate (`audio.sample_rate`) | `48000` Hz | Requested capture rate. This is a startup setting. |

### Spectral templates and detection

| Setting | Default | Meaning |
| --- | --- | --- |
| Template path (`template_path`) | `~/.config/pss2midi/pss-f30-templates.json` | File used to load Spectral examples and to save future calibration results. Template samples are kept separately from application settings. |
| Delay (`spectral.delay_ms`) | `8` ms | Time after a detected onset to begin the spectral sample window. |
| Window (`spectral.window_ms`) | `30` ms | Length of audio analyzed for a spectral match. Calibration and detection must use the same value. |
| Minimum score (`spectral.minimum_score`) | `0.75` | Lowest similarity score at which the best template can be accepted. Increase it to require a stronger match. |
| Minimum margin (`spectral.minimum_margin`) | `0.03` | Required score lead over the second-best template. Increase it to reject closer, more ambiguous matches. |

Scores and margins are between 0 and 1. Delay must be non-negative and Window
must be greater than zero. When Spectral or Compare is selected, FFT size must
be a power of two and at least as large as the number of samples in Window.

### Advanced controls

| Setting | Default | Meaning |
| --- | --- | --- |
| FFT size (`advanced.fft_size`) | `2048` | Number of points in the spectral transform. Must be a power of two; for Spectral or Compare it must cover the configured window. |
| Hop size (`advanced.hop_size`) | `128` samples | Number of audio samples processed between detector frames. Smaller hops update more often and use more processing. |
| Onset buffer (`advanced.onset_buffer_size`) | `1024` samples | Frame size used by onset detection. |
| Silence threshold (`advanced.silence_db`) | `-45` dBFS | Input level below which audio is treated as silence. |
| Release (`advanced.release_ms`) | `30` ms | Silence duration before sending MIDI Note Off. |
| Retrigger (`advanced.retrigger_ms`) | `60` ms | Minimum interval between repeated attacks of the same note. |
| Onset threshold (`advanced.onset_threshold`) | `0.30` | aubio peak threshold for live note-attack detection. Calibration uses a fixed onset threshold. |
| YIN pitch buffer (`advanced.pitch_buffer_size`) | `2048` samples | aubio/YIN pitch-analysis window size. |
| YIN attack ignore (`advanced.attack_ignore_ms`) | `10` ms | Time after an onset during which YIN pitch estimates are ignored, allowing the initial attack transient to pass. |
| YIN decision window (`advanced.decision_window_ms`) | `20` ms | Duration over which YIN collects pitch votes for a note decision. |
| YIN decision extension (`advanced.decision_extend_ms`) | `10` ms | Extra voting time used when the initial YIN votes are ambiguous. |
| YIN vote ratio (`advanced.vote_ratio`) | `0.60` | Fraction of valid YIN votes that must support the winning pitch. Range: 0–1. |
| YIN stable frames (`advanced.initial_stable_frames`) | `10` frames | Stable frames required for YIN's initial pitch-acquisition fallback when an onset is missed. Must be greater than zero. |

Release, Retrigger, spectral Delay, and YIN timing values must be non-negative.
Window must be positive. Hop size and Onset buffer must be greater than zero;
YIN pitch buffer and stable-frame count must be greater than zero in YIN or
Compare mode. The YIN timing, vote ratio, and spectral-specific constraints
apply when their respective detector is selected.

## Configuration file

The app reads and saves settings at `~/.config/pss2midi/config.json`. The
Settings page manages the options described above. One additional setting is
available only in the JSON file:

| JSON key | Default | Meaning |
| --- | --- | --- |
| `auto_start` | `false` | When set to `true`, the app attempts to start audio capture automatically on launch. |

For example, add `"auto_start": true` at the top level of the JSON object. The
application settings file and the spectral template file are separate.

## Troubleshooting

- **No input or no level activity:** check that the keyboard output is
  connected to the selected system input. Try **PipeWire default**, press
  **Refresh**, then **Retry input**.
- **Spectral is unavailable or has no matches:** calibrate the keyboard and
  check the template badge and template path. Recalibrate after changing
  sample rate, FFT size, spectral delay, or spectral window.
- **Calibration says too quiet or clipped:** adjust the audio interface or
  system input gain and play the requested note clearly. A clipped sample is
  rejected and must be captured again.
- **No MIDI reaches the other app:** make sure detection is running and select
  `PSS-F30 Audio MIDI` as the MIDI input in that app.
