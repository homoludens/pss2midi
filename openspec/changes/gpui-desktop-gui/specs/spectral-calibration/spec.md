## Purpose

Defines the guided process for collecting and saving Yamaha PSS-F30 spectral templates through the desktop application.

## ADDED Requirements

### Requirement: Calibration guides the user through all keyboard notes
The Calibration page SHALL guide the user through MIDI notes 36–72 in chromatic order, displaying the currently requested key and MIDI number. Calibration SHALL use the requested key rather than attempting to infer which key the user pressed. The default sample count SHALL be five accepted samples per note.

#### Scenario: Calibration begins
- **WHEN** the user starts calibration
- **THEN** the application requests C2 (MIDI 36), shows the first sample out of five, and highlights the current key

#### Scenario: Required samples for a note are accepted
- **WHEN** five samples for the requested note have been accepted
- **THEN** the application marks that note complete and advances to the next chromatic note

### Requirement: Calibration reports sample quality and supports retry
During calibration, the application SHALL show RMS and peak levels and report whether each captured sample is accepted. Quiet, clipped, or invalid captures SHALL be rejected or warned about, and a rejected capture SHALL NOT advance the sample or note progress. The user SHALL be able to retry the current sample or cancel calibration.

#### Scenario: Capture fails quality checks
- **WHEN** a captured sample is too quiet, clipped, or invalid
- **THEN** the application explains the quality issue and keeps the current note and sample index unchanged

#### Scenario: User retries the current sample
- **WHEN** the user selects Retry
- **THEN** the application requests another capture for the same note and sample index

#### Scenario: User cancels calibration
- **WHEN** the user selects Cancel during calibration
- **THEN** the active calibration workflow stops and the application leaves the user in control of the other pages and engine actions

### Requirement: Calibration progress is visible on the full keyboard
The Calibration page SHALL show the full 37-key keyboard, mark completed notes with a success indicator, highlight the current requested key in blue, and leave remaining notes in a neutral state. It SHALL show the current sample count and overall calibration progress.

#### Scenario: Calibration advances between notes
- **WHEN** a note is completed and the next note is requested
- **THEN** the completed key remains marked, the new current key is highlighted, and remaining keys stay neutral

### Requirement: Completed calibration saves templates and offers playback
After all 37 notes have the required samples, the application SHALL save the spectral templates to the configured template destination, defaulting to `~/.config/pss2midi/pss-f30-templates.json`. The completion view SHALL report 37 notes and 185 samples for the default sample count and provide a Start Playing action. Start Playing SHALL switch to Live, select Spectral mode, load the saved templates, and start the engine if it is stopped.

#### Scenario: All default calibration samples are complete
- **WHEN** the five required samples for every note from MIDI 36 through 72 have been accepted and saved
- **THEN** the application reports calibration complete, 37 notes, 185 samples, and the template destination

#### Scenario: User starts playing after calibration
- **WHEN** the user selects Start Playing from the completion view
- **THEN** the application opens Live in Spectral mode with the saved templates loaded and the engine running
