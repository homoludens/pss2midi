## Purpose

Defines how users control audio capture and detector operation and observe the availability of the audio and MIDI connections.

## ADDED Requirements

### Requirement: Start and stop control the audio-to-MIDI engine
The application SHALL provide a Start or Stop action reflecting the engine's actual running state. Starting SHALL open the selected audio input and resume detection and MIDI output. Stopping SHALL stop capture and detector activity, send note-off for any active MIDI note, and release audio resources. Closing the application SHALL perform equivalent clean shutdown so no MIDI note remains hanging.

#### Scenario: User starts a stopped engine
- **WHEN** the user selects Start and the audio input can be opened
- **THEN** capture and detection begin and the UI reports the engine as running

#### Scenario: User stops while a note is active
- **WHEN** the user selects Stop while a MIDI note is active
- **THEN** the engine sends note-off, stops capture and detection, releases audio resources, and the UI reports it as stopped

#### Scenario: Application closes while running
- **WHEN** the user closes the application while capture or a MIDI note is active
- **THEN** capture and detection shut down and any active MIDI note is released before resources are closed

### Requirement: Users can select and recover audio input devices
The application SHALL enumerate available capture devices and allow the user to select an input. It SHALL prefer a previously saved device and otherwise use the PipeWire default input. If the selected device is unavailable or cannot be opened, the application SHALL show an in-app error and offer a retry action without crashing.

#### Scenario: No previously saved input exists
- **WHEN** the application initializes and no saved audio device is available
- **THEN** it selects or offers the PipeWire default input

#### Scenario: Selected input becomes unavailable
- **WHEN** the selected device disappears or fails to open
- **THEN** the application reports that the audio device is unavailable and offers Retry without terminating the application

### Requirement: Detector mode determines the MIDI-driving result
The application SHALL support YIN, Spectral, and Compare modes. YIN mode SHALL use the YIN result to drive MIDI; Spectral mode SHALL use the spectral result; Compare mode SHALL run both detectors, use the spectral result to drive MIDI, and expose disagreement. A mode change while running SHALL take effect safely, restarting the engine if necessary without requiring an application restart.

#### Scenario: User changes detector mode while running
- **WHEN** the user selects another detector mode during capture
- **THEN** the new mode becomes active, either directly or after an automatic engine restart, without restarting the application

#### Scenario: Spectral templates are unavailable
- **WHEN** the user selects Spectral or Compare mode and no valid templates are loaded
- **THEN** the application reports that calibration is needed and provides access to calibration while keeping YIN available

### Requirement: MIDI output status is visible
The application SHALL initialize and report the status of the virtual MIDI output named `PSS-F30 Audio MIDI`. If output creation fails, it SHALL show an in-app error instead of presenting the output as connected.

#### Scenario: MIDI output is created
- **WHEN** the MIDI output is available
- **THEN** the UI identifies `PSS-F30 Audio MIDI` and reports it as connected

#### Scenario: MIDI output creation fails
- **WHEN** the MIDI output cannot be initialized
- **THEN** the UI reports the failure and does not show a connected status
