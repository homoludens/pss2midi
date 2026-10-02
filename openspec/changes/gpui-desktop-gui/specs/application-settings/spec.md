## Purpose

Defines the desktop settings users can configure and how those settings and templates are handled across application launches.

## ADDED Requirements

### Requirement: Settings are configurable and persist across launches
The Settings page SHALL allow users to configure the audio input and sample rate, detector mode, template path, and spectral parameters including delay, window, minimum score, and minimum margin. Advanced controls SHALL be collapsed by default and expose the configured FFT, silence, release, retrigger, onset, YIN, and spectral tuning options. The application SHALL persist settings in `~/.config/pss2midi/config.json` and restore saved values on the next launch.

#### Scenario: User changes a setting
- **WHEN** the user changes a setting and the application saves configuration
- **THEN** the chosen value is restored the next time the application launches

#### Scenario: Advanced settings are initially hidden
- **WHEN** the user opens Settings with no explicit advanced expansion
- **THEN** advanced controls are collapsed while common audio and detector settings remain visible

### Requirement: Missing configuration and templates use safe defaults
If the configuration file does not exist, the application SHALL use safe defaults and remain usable. If spectral templates are missing or invalid, the application SHALL allow YIN operation and report that spectral detection is not calibrated, with an action to start calibration. Missing templates SHALL NOT prevent application startup.

#### Scenario: First launch without a configuration file
- **WHEN** the application starts and `~/.config/pss2midi/config.json` is absent
- **THEN** it initializes with safe default settings and opens the Live page

#### Scenario: Templates are missing
- **WHEN** the application starts without a valid spectral template file
- **THEN** it continues startup with YIN available and presents Spectral as not calibrated with a path to calibration

### Requirement: Application startup restores saved preferences
On launch, the application SHALL load saved configuration, initialize the desktop interface, enumerate audio devices, and load spectral templates when available. It SHALL initialize MIDI output and open the Live page. If saved configuration requests automatic engine startup, the application SHALL attempt to start capture; otherwise, it SHALL remain stopped and offer Start.

#### Scenario: Saved configuration requests automatic start
- **WHEN** the application starts with automatic engine startup enabled in its saved configuration
- **THEN** it attempts to start the audio-to-MIDI engine after initialization

#### Scenario: Automatic startup is not enabled
- **WHEN** the application starts without an automatic-start preference
- **THEN** it opens Live in the stopped state and offers a Start action
