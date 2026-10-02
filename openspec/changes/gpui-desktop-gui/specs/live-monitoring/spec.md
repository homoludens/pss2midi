## Purpose

Defines the live feedback users rely on to monitor audio-to-MIDI detection, compare detector results, and understand which notes are active.

## ADDED Requirements

### Requirement: Live view reports current detection and audio levels
The Live view SHALL show the selected detected note as a note name and MIDI number, latency when available, and the latest input RMS and peak levels. Displayed values SHALL come from engine updates; when a value is unavailable, the UI SHALL indicate that it is unavailable rather than showing a fabricated measurement.

#### Scenario: A note is detected
- **WHEN** the engine reports a selected note and current level measurements
- **THEN** the Live view shows the note name, MIDI number, input level, and peak level
- **AND** it shows the reported latency when one is available

#### Scenario: No note or latency is available
- **WHEN** the engine has no selected note or has not reported latency
- **THEN** the Live view presents an idle or unavailable state for those values

### Requirement: Detector results and spectral rankings are visible
The Live view SHALL present available YIN and spectral results, including spectral confidence and margin when supplied. It SHALL show up to the three best spectral matches using actual detector classifications and scores. When both detectors report notes, the view SHALL distinguish agreement from disagreement and identify the authoritative result for the active mode.

#### Scenario: Detectors disagree in compare mode
- **WHEN** YIN and spectral detection report different notes in Compare mode
- **THEN** the Live view shows both results and marks their disagreement with a warning state
- **AND** it identifies the spectral result as authoritative for MIDI output

#### Scenario: Spectral detector reports ranked matches
- **WHEN** the spectral detector provides a classification, confidence, margin, and ranked matches
- **THEN** the Live view shows those values and the three highest-ranked matches with their scores
- **AND** marks the spectral result as selected when Spectral mode is active

### Requirement: Keyboard visualization maps the supported MIDI range
The Live view SHALL display a correctly ordered, display-only piano keyboard covering MIDI notes 36 through 72 inclusive, with white and black keys positioned according to their pitches and octave labels C2, C3, C4, and C5. It SHALL highlight the selected note in blue and indicate a differing YIN note with an amber marker or outline.

#### Scenario: A detected note is highlighted
- **WHEN** the engine reports a selected MIDI note within 36–72
- **THEN** the corresponding key is highlighted in blue without shifting the mapping of other keys

#### Scenario: YIN disagrees with the selected note
- **WHEN** the YIN result differs from the selected result
- **THEN** the keyboard marks the YIN note in amber while retaining the selected note's blue highlight

### Requirement: Recent detection events can be reviewed
The application SHALL retain approximately the latest 100 high-level events and display a compact list of 6–12 recent events when the log is enabled. The list SHALL include event time and a concise description of onset, note, detector, or note-off events as applicable. Users SHALL be able to hide or show the event list.

#### Scenario: New events arrive while the log is visible
- **WHEN** onset, detection, or note-off events arrive
- **THEN** the visible list updates with recent timestamped events and older entries are discarded as the retained history reaches its limit

#### Scenario: User hides the event log
- **WHEN** the user disables Show log
- **THEN** the recent-event panel is hidden without affecting audio detection or MIDI output
