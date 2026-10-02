//! Plain application state and engine-event reduction for the desktop UI.

use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use crate::engine::{
    app_config::AppConfig,
    calibration::{CalibrationNoteCompletion, CalibrationProgress, CalibrationSampleQuality},
    config::DetectorMode,
    detector::{SpectralResult, YinResult},
    note::note_name,
    AudioInputDevice, EngineCommand, EngineEvent, EngineState,
};

/// Maximum number of high-level events retained for the recent-event panel.
pub const RECENT_EVENT_LIMIT: usize = 100;

/// At most 25 level-state updates per second. Semantic events are never delayed.
pub const LEVEL_UPDATE_INTERVAL: Duration = Duration::from_millis(40);

pub const CALIBRATION_FIRST_NOTE: u8 = 36;
pub const CALIBRATION_LAST_NOTE: u8 = 72;
pub const CALIBRATION_NOTE_COUNT: usize =
    (CALIBRATION_LAST_NOTE - CALIBRATION_FIRST_NOTE + 1) as usize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Live,
    Calibration,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Runtime,
    AudioDevice,
    AudioDeviceEnumeration,
    MidiOutput,
    Calibration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiOutputStatus {
    NotInitialized,
    Available { name: String },
    Closed,
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveControlAction {
    ToggleEngine,
    EnumerateAudioDevices,
    SelectAudioDevice { device_id: String },
    RetryAudioDevice,
    SetDetectorMode(DetectorMode),
}

/// Map a UI action to an engine command without changing UI state. In
/// particular, toggle behavior is based on the last engine-reported state.
pub fn engine_command_for_action(
    action: LiveControlAction,
    engine_state: EngineState,
) -> Option<EngineCommand> {
    match action {
        LiveControlAction::ToggleEngine => match engine_state {
            EngineState::Stopped => Some(EngineCommand::Start),
            EngineState::Running => Some(EngineCommand::Stop),
            EngineState::ShuttingDown => None,
        },
        LiveControlAction::EnumerateAudioDevices => Some(EngineCommand::EnumerateAudioDevices),
        LiveControlAction::SelectAudioDevice { device_id } => {
            Some(EngineCommand::SetAudioDevice { device: device_id })
        }
        LiveControlAction::RetryAudioDevice => Some(EngineCommand::RetryAudioDevice),
        LiveControlAction::SetDetectorMode(mode) => Some(EngineCommand::SetDetectorMode(mode)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorBanner {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioLevels {
    pub rms_dbfs: f32,
    pub peak_dbfs: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalibrationStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibrationCompletion {
    pub note_count: usize,
    pub sample_count: usize,
    pub template_path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalibrationState {
    pub status: CalibrationStatus,
    pub progress: Option<CalibrationProgress>,
    pub completed_notes: BTreeSet<u8>,
    pub completion: Option<CalibrationCompletion>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectionSnapshot {
    pub selected_note: Option<u8>,
    pub yin_note: Option<u8>,
    pub spectral_note: Option<u8>,
    pub spectral_confidence: Option<f32>,
    pub onset_to_note_latency: Option<Duration>,
}

impl DetectionSnapshot {
    fn description(self) -> String {
        let mut parts = vec![self
            .selected_note
            .map(note_name)
            .unwrap_or_else(|| "No note".to_owned())];

        match (self.spectral_note, self.spectral_confidence) {
            (Some(note), Some(confidence)) if Some(note) != self.selected_note => {
                parts.push(format!(
                    "spectral={} ({})",
                    note_name(note),
                    format_confidence(confidence)
                ));
            }
            (_, Some(confidence)) => {
                parts.push(format!("spectral={}", format_confidence(confidence)));
            }
            (Some(note), None) => parts.push(format!("spectral={}", note_name(note))),
            (None, None) => {}
        }

        if let Some(note) = self.yin_note {
            parts.push(format!("YIN={}", note_name(note)));
        }
        if let Some(latency) = self.onset_to_note_latency {
            parts.push(format!("{}ms", latency.as_millis()));
        }

        parts.join(" ")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecentEventKind {
    WorkerStarted,
    WorkerStopped,
    EngineStateChanged(EngineState),
    DetectorModeChanged(DetectorMode),
    MidiOutputOpened {
        name: String,
    },
    MidiOutputClosed,
    MidiOutputUnavailable {
        message: String,
    },
    Onset,
    Detection(DetectionSnapshot),
    NoteOn {
        midi_note: u8,
    },
    NoteOff {
        midi_note: u8,
    },
    AudioDevicesEnumerated {
        device_count: usize,
        selected_device: String,
    },
    AudioDeviceSelected {
        device_id: String,
    },
    AudioDeviceOpened {
        device_id: String,
    },
    AudioDeviceUnavailable {
        device_id: String,
        message: String,
    },
    AudioDeviceEnumerationFailed {
        message: String,
    },
    CalibrationProgress {
        requested_note: u8,
        sample_index: usize,
        samples_per_note: usize,
        quality: CalibrationSampleQuality,
    },
    CalibrationNoteCompleted(CalibrationNoteCompletion),
    CalibrationCompleted {
        note_count: usize,
        sample_count: usize,
        template_path: PathBuf,
    },
    CalibrationCancelled,
    CalibrationError {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentEvent {
    /// Wall-clock event time for time-of-day displays; meter throttling uses a
    /// separate monotonic `Instant` and never relies on this value.
    pub wall_clock_at: SystemTime,
    pub kind: RecentEventKind,
}

impl RecentEvent {
    /// Human-readable, compact text suitable for the recent-event panel.
    pub fn description(&self) -> String {
        match &self.kind {
            RecentEventKind::WorkerStarted => "Engine worker started".to_owned(),
            RecentEventKind::WorkerStopped => "Engine worker stopped".to_owned(),
            RecentEventKind::EngineStateChanged(state) => {
                format!("Engine {}", engine_state_label(*state))
            }
            RecentEventKind::DetectorModeChanged(mode) => {
                format!("Detector mode changed to {}", detector_mode_label(*mode))
            }
            RecentEventKind::MidiOutputOpened { name } => {
                format!("MIDI output connected: {name}")
            }
            RecentEventKind::MidiOutputClosed => "MIDI output closed".to_owned(),
            RecentEventKind::MidiOutputUnavailable { message } => {
                format!("MIDI output unavailable: {message}")
            }
            RecentEventKind::Onset => "Onset detected".to_owned(),
            RecentEventKind::Detection(snapshot) => snapshot.description(),
            RecentEventKind::NoteOn { midi_note } => {
                format!("Note on: {} (MIDI {midi_note})", note_name(*midi_note))
            }
            RecentEventKind::NoteOff { midi_note } => {
                format!("Note off: {} (MIDI {midi_note})", note_name(*midi_note))
            }
            RecentEventKind::AudioDevicesEnumerated {
                device_count,
                selected_device,
            } => format!("Found {device_count} audio inputs; selected {selected_device}"),
            RecentEventKind::AudioDeviceSelected { device_id } => {
                format!("Selected audio input {device_id}")
            }
            RecentEventKind::AudioDeviceOpened { device_id } => {
                format!("Opened audio input {device_id}")
            }
            RecentEventKind::AudioDeviceUnavailable { device_id, message } => {
                format!("Audio input {device_id} unavailable: {message}")
            }
            RecentEventKind::AudioDeviceEnumerationFailed { message } => {
                format!("Could not enumerate audio inputs: {message}")
            }
            RecentEventKind::CalibrationProgress {
                requested_note,
                sample_index,
                samples_per_note,
                quality,
            } => format!(
                "Calibration {} sample {sample_index}/{samples_per_note}: {}",
                note_name(*requested_note),
                calibration_quality_label(quality)
            ),
            RecentEventKind::CalibrationNoteCompleted(note) => format!(
                "Calibration note completed: {} ({}/{})",
                note_name(note.midi_note),
                note.completed_notes,
                note.total_notes
            ),
            RecentEventKind::CalibrationCompleted {
                note_count,
                sample_count,
                ..
            } => format!("Calibration completed: {note_count} notes, {sample_count} samples"),
            RecentEventKind::CalibrationCancelled => "Calibration cancelled".to_owned(),
            RecentEventKind::CalibrationError { message } => {
                format!("Calibration error: {message}")
            }
            RecentEventKind::Error { message } => format!("Error: {message}"),
        }
    }
}

/// UI-facing state. DSP, capture, and MIDI resources remain owned by the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub page: Page,
    pub config: AppConfig,
    pub engine_state: EngineState,
    pub worker_running: bool,
    pub midi_output_status: MidiOutputStatus,
    pub selected_note: Option<u8>,
    pub active_note: Option<u8>,
    pub yin_result: Option<YinResult>,
    pub spectral_result: Option<SpectralResult>,
    pub onset_to_note_latency: Option<Duration>,
    pub audio_levels: Option<AudioLevels>,
    pub audio_devices: Vec<AudioInputDevice>,
    pub selected_audio_device: String,
    pub audio_device_open: bool,
    pub calibration: CalibrationState,
    pub error_banner: Option<ErrorBanner>,
    pub recent_events: VecDeque<RecentEvent>,
    last_level_update_at: Option<Instant>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(AppConfig::default())
    }
}

impl AppState {
    /// Create state from the loaded (or safe-default) application settings.
    pub fn new(config: AppConfig) -> Self {
        Self {
            page: Page::Live,
            selected_audio_device: config.audio.device.clone(),
            config,
            engine_state: EngineState::Stopped,
            worker_running: false,
            midi_output_status: MidiOutputStatus::NotInitialized,
            selected_note: None,
            active_note: None,
            yin_result: None,
            spectral_result: None,
            onset_to_note_latency: None,
            audio_levels: None,
            audio_devices: Vec::new(),
            audio_device_open: false,
            calibration: CalibrationState::default(),
            error_banner: None,
            recent_events: VecDeque::with_capacity(RECENT_EVENT_LIMIT),
            last_level_update_at: None,
        }
    }

    /// Select the UI page requested by the shell navigation.
    pub fn select_page(&mut self, page: Page) {
        self.page = page;
    }

    /// Apply an engine update using a monotonic meter timestamp and the current
    /// wall clock for timestamping semantic event history.
    ///
    /// Suppressed audio-level events only affect the meter state; all semantic
    /// events are reduced and appended to history immediately.
    pub fn reduce_event(&mut self, event: EngineEvent, now: Instant) {
        self.reduce_event_at(event, now, SystemTime::now());
    }

    /// Apply an event with both clocks supplied by the caller. This is useful
    /// for deterministic tests and keeps wall-clock display time independent
    /// of monotonic meter coalescing.
    pub fn reduce_event_at(
        &mut self,
        event: EngineEvent,
        monotonic_now: Instant,
        wall_clock_at: SystemTime,
    ) {
        match event {
            EngineEvent::WorkerStarted => {
                self.worker_running = true;
                self.push_event(RecentEventKind::WorkerStarted, wall_clock_at);
            }
            EngineEvent::WorkerStopped => {
                self.worker_running = false;
                self.engine_state = EngineState::Stopped;
                self.clear_active_note();
                self.push_event(RecentEventKind::WorkerStopped, wall_clock_at);
            }
            EngineEvent::StateChanged(state) => {
                self.engine_state = state;
                if matches!(state, EngineState::Stopped | EngineState::ShuttingDown) {
                    self.clear_active_note();
                    self.audio_device_open = false;
                }
                self.push_event(RecentEventKind::EngineStateChanged(state), wall_clock_at);
            }
            EngineEvent::DetectorModeChanged(mode) => {
                self.config.detector_mode = mode;
                self.push_event(RecentEventKind::DetectorModeChanged(mode), wall_clock_at);
            }
            EngineEvent::MidiOutputOpened { name } => {
                self.midi_output_status = MidiOutputStatus::Available { name: name.clone() };
                self.clear_error_of_kind(ErrorKind::MidiOutput);
                self.push_event(RecentEventKind::MidiOutputOpened { name }, wall_clock_at);
            }
            EngineEvent::MidiOutputClosed => {
                self.midi_output_status = MidiOutputStatus::Closed;
                self.push_event(RecentEventKind::MidiOutputClosed, wall_clock_at);
            }
            EngineEvent::MidiOutputUnavailable { message } => {
                self.midi_output_status = MidiOutputStatus::Error {
                    message: message.clone(),
                };
                self.set_error(ErrorKind::MidiOutput, message.clone());
                self.push_event(
                    RecentEventKind::MidiOutputUnavailable { message },
                    wall_clock_at,
                );
            }
            EngineEvent::Onset => self.push_event(RecentEventKind::Onset, wall_clock_at),
            EngineEvent::Detection {
                selected_note,
                yin,
                spectral,
                onset_to_note_latency,
            } => {
                let snapshot = DetectionSnapshot {
                    selected_note,
                    yin_note: yin.as_ref().and_then(|result| result.note),
                    spectral_note: spectral.as_ref().map(|result| result.note),
                    spectral_confidence: spectral.as_ref().map(|result| result.confidence),
                    onset_to_note_latency,
                };
                self.selected_note = selected_note;
                self.yin_result = yin;
                self.spectral_result = spectral;
                self.onset_to_note_latency = onset_to_note_latency;
                self.push_event(RecentEventKind::Detection(snapshot), wall_clock_at);
            }
            EngineEvent::NoteOn { midi_note } => {
                self.selected_note = Some(midi_note);
                self.active_note = Some(midi_note);
                self.push_event(RecentEventKind::NoteOn { midi_note }, wall_clock_at);
            }
            EngineEvent::NoteOff { midi_note } => {
                if self.active_note == Some(midi_note) {
                    self.active_note = None;
                }
                if self.selected_note == Some(midi_note) {
                    self.selected_note = None;
                }
                self.push_event(RecentEventKind::NoteOff { midi_note }, wall_clock_at);
            }
            EngineEvent::AudioLevel {
                rms_dbfs,
                peak_dbfs,
            } => {
                let should_update = self.last_level_update_at.map_or(true, |last| {
                    monotonic_now.saturating_duration_since(last) >= LEVEL_UPDATE_INTERVAL
                });
                if should_update {
                    self.audio_levels = Some(AudioLevels {
                        rms_dbfs,
                        peak_dbfs,
                    });
                    self.last_level_update_at = Some(monotonic_now);
                }
            }
            EngineEvent::AudioDevicesEnumerated {
                devices,
                selected_device,
            } => {
                let device_count = devices.len();
                self.audio_devices = devices;
                self.set_selected_audio_device(selected_device.clone());
                self.error_banner = self
                    .error_banner
                    .take()
                    .filter(|banner| banner.kind != ErrorKind::AudioDeviceEnumeration);
                self.push_event(
                    RecentEventKind::AudioDevicesEnumerated {
                        device_count,
                        selected_device,
                    },
                    wall_clock_at,
                );
            }
            EngineEvent::AudioDeviceSelected { device_id } => {
                let device_changed = self.selected_audio_device != device_id;
                self.set_selected_audio_device(device_id.clone());
                if device_changed {
                    self.audio_device_open = false;
                }
                self.push_event(
                    RecentEventKind::AudioDeviceSelected { device_id },
                    wall_clock_at,
                );
            }
            EngineEvent::AudioDeviceOpened { device_id } => {
                self.set_selected_audio_device(device_id.clone());
                self.audio_device_open = true;
                self.clear_error_of_kind(ErrorKind::AudioDevice);
                self.push_event(
                    RecentEventKind::AudioDeviceOpened { device_id },
                    wall_clock_at,
                );
            }
            EngineEvent::AudioDeviceUnavailable { device_id, message } => {
                self.set_selected_audio_device(device_id.clone());
                self.audio_device_open = false;
                self.set_error(ErrorKind::AudioDevice, message.clone());
                self.push_event(
                    RecentEventKind::AudioDeviceUnavailable { device_id, message },
                    wall_clock_at,
                );
            }
            EngineEvent::AudioDeviceEnumerationFailed { message } => {
                self.set_error(ErrorKind::AudioDeviceEnumeration, message.clone());
                self.push_event(
                    RecentEventKind::AudioDeviceEnumerationFailed { message },
                    wall_clock_at,
                );
            }
            EngineEvent::CalibrationProgress(progress) => {
                if self.calibration.status != CalibrationStatus::Running {
                    self.calibration.completed_notes.clear();
                }
                self.calibration.status = CalibrationStatus::Running;
                self.calibration.progress = Some(progress.clone());
                self.calibration.completion = None;
                self.push_event(
                    RecentEventKind::CalibrationProgress {
                        requested_note: progress.requested_note,
                        sample_index: progress.sample_index,
                        samples_per_note: progress.samples_per_note,
                        quality: progress.quality,
                    },
                    wall_clock_at,
                );
            }
            EngineEvent::CalibrationNoteCompleted(note) => {
                self.calibration.status = CalibrationStatus::Running;
                self.calibration.completed_notes.insert(note.midi_note);
                self.push_event(
                    RecentEventKind::CalibrationNoteCompleted(note),
                    wall_clock_at,
                );
            }
            EngineEvent::CalibrationCompleted {
                note_count,
                sample_count,
                template_path,
            } => {
                self.calibration.status = CalibrationStatus::Completed;
                self.calibration.completion = Some(CalibrationCompletion {
                    note_count,
                    sample_count,
                    template_path: template_path.clone(),
                });
                self.clear_error_of_kind(ErrorKind::Calibration);
                self.push_event(
                    RecentEventKind::CalibrationCompleted {
                        note_count,
                        sample_count,
                        template_path,
                    },
                    wall_clock_at,
                );
            }
            EngineEvent::CalibrationCancelled => {
                self.calibration.status = CalibrationStatus::Cancelled;
                self.calibration.completion = None;
                self.push_event(RecentEventKind::CalibrationCancelled, wall_clock_at);
            }
            EngineEvent::CalibrationError { message } => {
                self.calibration.status = CalibrationStatus::Error;
                self.set_error(ErrorKind::Calibration, message.clone());
                self.push_event(RecentEventKind::CalibrationError { message }, wall_clock_at);
            }
            EngineEvent::Error { message } => {
                self.set_error(ErrorKind::Runtime, message.clone());
                self.push_event(RecentEventKind::Error { message }, wall_clock_at);
            }
        }
    }

    pub fn clear_error_banner(&mut self) {
        self.error_banner = None;
    }

    fn clear_active_note(&mut self) {
        self.active_note = None;
        self.selected_note = None;
    }

    fn set_selected_audio_device(&mut self, device_id: String) {
        self.selected_audio_device = device_id.clone();
        self.config.audio.device = device_id;
    }

    fn set_error(&mut self, kind: ErrorKind, message: String) {
        self.error_banner = Some(ErrorBanner { kind, message });
    }

    fn clear_error_of_kind(&mut self, kind: ErrorKind) {
        if self
            .error_banner
            .as_ref()
            .is_some_and(|banner| banner.kind == kind)
        {
            self.error_banner = None;
        }
    }

    fn push_event(&mut self, kind: RecentEventKind, wall_clock_at: SystemTime) {
        if self.recent_events.len() == RECENT_EVENT_LIMIT {
            self.recent_events.pop_front();
        }
        self.recent_events.push_back(RecentEvent {
            wall_clock_at,
            kind,
        });
    }
}

fn engine_state_label(state: EngineState) -> &'static str {
    match state {
        EngineState::Stopped => "stopped",
        EngineState::Running => "running",
        EngineState::ShuttingDown => "shutting down",
    }
}

fn detector_mode_label(mode: DetectorMode) -> &'static str {
    match mode {
        DetectorMode::Yin => "YIN",
        DetectorMode::Spectral => "Spectral",
        DetectorMode::Compare => "Compare",
    }
}

fn calibration_quality_label(quality: &CalibrationSampleQuality) -> &'static str {
    match quality {
        CalibrationSampleQuality::AwaitingOnset => "waiting for onset",
        CalibrationSampleQuality::Accepted => "accepted",
        CalibrationSampleQuality::Rejected(reason) => match reason {
            crate::engine::calibration::CalibrationRejectionReason::TooQuiet => "too quiet",
            crate::engine::calibration::CalibrationRejectionReason::Clipped => "clipped",
            crate::engine::calibration::CalibrationRejectionReason::Invalid => "invalid",
        },
    }
}

fn format_confidence(confidence: f32) -> String {
    let formatted = format!("{confidence:.2}");
    formatted
        .strip_prefix("0.")
        .map(|fraction| format!(".{fraction}"))
        .unwrap_or(formatted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{
        calibration::{
            CalibrationNoteCompletion, CalibrationProgress, CalibrationRejectionReason,
            CalibrationSampleQuality,
        },
        detector::{SpectralResult, YinDecision, YinResult},
        templates::RankedMatch,
    };

    fn at(milliseconds: u64) -> Instant {
        static TEST_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *TEST_EPOCH.get_or_init(Instant::now) + Duration::from_millis(milliseconds)
    }

    fn wall_at(milliseconds: u64) -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_millis(milliseconds)
    }

    fn progress(quality: CalibrationSampleQuality) -> CalibrationProgress {
        CalibrationProgress {
            requested_note: 36,
            sample_index: 2,
            samples_per_note: 5,
            accepted_samples_for_note: 1,
            accepted_samples: 1,
            required_samples: 185,
            completed_notes: 0,
            rms_dbfs: Some(-18.0),
            peak: Some(0.5),
            quality,
        }
    }

    #[test]
    fn initial_state_uses_loaded_settings_and_opens_on_live_page() {
        let mut config = AppConfig::default();
        config.audio.device = "hw:2,0".to_owned();
        config.audio.sample_rate = 44_100;
        config.detector_mode = crate::engine::config::DetectorMode::Compare;

        let state = AppState::new(config.clone());

        assert_eq!(state.page, Page::Live);
        assert_eq!(state.config, config);
        assert_eq!(state.selected_audio_device, "hw:2,0");
        assert_eq!(state.engine_state, EngineState::Stopped);
    }

    #[test]
    fn page_selection_updates_state_for_each_navigation_target() {
        let mut state = AppState::default();
        assert_eq!(state.page, Page::Live);

        for page in [Page::Calibration, Page::Settings, Page::Live] {
            state.select_page(page);
            assert_eq!(state.page, page);
        }
    }

    #[test]
    fn live_control_actions_map_to_engine_commands_from_reported_state() {
        assert_eq!(
            engine_command_for_action(LiveControlAction::ToggleEngine, EngineState::Stopped),
            Some(EngineCommand::Start)
        );
        assert_eq!(
            engine_command_for_action(LiveControlAction::ToggleEngine, EngineState::Running),
            Some(EngineCommand::Stop)
        );
        assert_eq!(
            engine_command_for_action(LiveControlAction::ToggleEngine, EngineState::ShuttingDown),
            None
        );
        assert_eq!(
            engine_command_for_action(
                LiveControlAction::EnumerateAudioDevices,
                EngineState::Stopped
            ),
            Some(EngineCommand::EnumerateAudioDevices)
        );
        assert_eq!(
            engine_command_for_action(
                LiveControlAction::SelectAudioDevice {
                    device_id: "hw:2,0".to_owned(),
                },
                EngineState::Running
            ),
            Some(EngineCommand::SetAudioDevice {
                device: "hw:2,0".to_owned(),
            })
        );
        assert_eq!(
            engine_command_for_action(LiveControlAction::RetryAudioDevice, EngineState::Stopped),
            Some(EngineCommand::RetryAudioDevice)
        );
        for mode in [
            DetectorMode::Yin,
            DetectorMode::Spectral,
            DetectorMode::Compare,
        ] {
            assert_eq!(
                engine_command_for_action(
                    LiveControlAction::SetDetectorMode(mode),
                    EngineState::Stopped
                ),
                Some(EngineCommand::SetDetectorMode(mode))
            );
        }
    }

    #[test]
    fn midi_and_mode_controls_follow_engine_events_instead_of_assuming_success() {
        let mut state = AppState::default();
        assert_eq!(state.midi_output_status, MidiOutputStatus::NotInitialized);

        state.reduce_event(EngineEvent::StateChanged(EngineState::Running), at(0));
        assert_eq!(state.midi_output_status, MidiOutputStatus::NotInitialized);

        state.reduce_event(
            EngineEvent::MidiOutputOpened {
                name: "PSS-F30 Audio MIDI".to_owned(),
            },
            at(1),
        );
        assert_eq!(
            state.midi_output_status,
            MidiOutputStatus::Available {
                name: "PSS-F30 Audio MIDI".to_owned(),
            }
        );

        state.reduce_event(
            EngineEvent::DetectorModeChanged(DetectorMode::Compare),
            at(2),
        );
        assert_eq!(state.config.detector_mode, DetectorMode::Compare);

        state.reduce_event(
            EngineEvent::MidiOutputUnavailable {
                message: "MIDI backend unavailable".to_owned(),
            },
            at(3),
        );
        assert_eq!(
            state.midi_output_status,
            MidiOutputStatus::Error {
                message: "MIDI backend unavailable".to_owned(),
            }
        );
        assert_eq!(
            state.error_banner,
            Some(ErrorBanner {
                kind: ErrorKind::MidiOutput,
                message: "MIDI backend unavailable".to_owned(),
            })
        );

        state.reduce_event(EngineEvent::MidiOutputClosed, at(4));
        assert_eq!(state.midi_output_status, MidiOutputStatus::Closed);
    }

    #[test]
    fn note_on_and_note_off_update_selected_and_active_note() {
        let mut state = AppState::default();
        state.reduce_event(EngineEvent::NoteOn { midi_note: 60 }, at(0));

        assert_eq!(state.selected_note, Some(60));
        assert_eq!(state.active_note, Some(60));
        assert_eq!(state.recent_events.len(), 1);
        assert_eq!(
            state.recent_events[0].description(),
            "Note on: C4 (MIDI 60)"
        );

        state.reduce_event(EngineEvent::NoteOff { midi_note: 60 }, at(1));
        assert_eq!(state.selected_note, None);
        assert_eq!(state.active_note, None);
        assert!(matches!(
            state.recent_events.back().map(|event| &event.kind),
            Some(RecentEventKind::NoteOff { midi_note: 60 })
        ));
    }

    #[test]
    fn detection_reduces_current_results_and_keeps_historical_event_snapshot() {
        let mut state = AppState::default();
        let latency = Duration::from_millis(30);
        let yin = YinResult {
            midi_pitch: Some(55.1),
            note: Some(55),
            cents: Some(10.0),
            decision: Some(YinDecision {
                candidate_note: Some(55),
                accepted: true,
                vote_count: 4,
                total_votes: 5,
                vote_ratio: Some(0.8),
            }),
        };
        let spectral = SpectralResult {
            note: 48,
            confidence: 0.94,
            second_score: 0.4,
            margin: 0.54,
            accepted: true,
            selected_note: Some(48),
            ranked_matches: vec![RankedMatch {
                note: 48,
                score: 0.94,
            }],
        };

        state.reduce_event_at(
            EngineEvent::Detection {
                selected_note: Some(48),
                yin: Some(yin.clone()),
                spectral: Some(spectral.clone()),
                onset_to_note_latency: Some(latency),
            },
            at(0),
            wall_at(0),
        );

        assert_eq!(state.selected_note, Some(48));
        assert_eq!(state.yin_result, Some(yin));
        assert_eq!(state.spectral_result, Some(spectral));
        assert_eq!(state.onset_to_note_latency, Some(latency));

        let first_detection = state.recent_events.back().unwrap();
        assert_eq!(
            first_detection.kind,
            RecentEventKind::Detection(DetectionSnapshot {
                selected_note: Some(48),
                yin_note: Some(55),
                spectral_note: Some(48),
                spectral_confidence: Some(0.94),
                onset_to_note_latency: Some(latency),
            })
        );
        assert_eq!(first_detection.description(), "C3 spectral=.94 YIN=G3 30ms");

        state.reduce_event_at(
            EngineEvent::Detection {
                selected_note: Some(60),
                yin: None,
                spectral: None,
                onset_to_note_latency: None,
            },
            at(1),
            wall_at(1),
        );
        assert_eq!(state.selected_note, Some(60));
        assert_eq!(state.yin_result, None);
        assert_eq!(
            state.recent_events.front().unwrap().description(),
            "C3 spectral=.94 YIN=G3 30ms"
        );
    }

    #[test]
    fn recent_event_retains_injected_wall_clock_timestamp() {
        let mut state = AppState::default();
        let wall_clock_at = wall_at(12_345_678);

        state.reduce_event_at(EngineEvent::Onset, at(0), wall_clock_at);

        assert_eq!(state.recent_events[0].wall_clock_at, wall_clock_at);
        assert_eq!(
            state.recent_events[0]
                .wall_clock_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap(),
            Duration::from_millis(12_345_678)
        );
    }

    #[test]
    fn lifecycle_events_track_worker_and_engine_state_and_release_notes() {
        let mut state = AppState::default();
        state.reduce_event(EngineEvent::WorkerStarted, at(0));
        state.reduce_event(EngineEvent::StateChanged(EngineState::Running), at(1));
        state.reduce_event(EngineEvent::NoteOn { midi_note: 60 }, at(2));

        assert!(state.worker_running);
        assert_eq!(state.engine_state, EngineState::Running);
        assert_eq!(state.active_note, Some(60));

        state.reduce_event(EngineEvent::StateChanged(EngineState::Stopped), at(3));
        assert_eq!(state.engine_state, EngineState::Stopped);
        assert_eq!(state.active_note, None);
        assert_eq!(state.selected_note, None);

        state.reduce_event(EngineEvent::WorkerStopped, at(4));
        assert!(!state.worker_running);
        assert_eq!(state.engine_state, EngineState::Stopped);
    }

    #[test]
    fn calibration_progress_completion_cancel_and_error_are_reduced() {
        let mut state = AppState::default();
        let rejected = progress(CalibrationSampleQuality::Rejected(
            CalibrationRejectionReason::TooQuiet,
        ));
        state.reduce_event(EngineEvent::CalibrationProgress(rejected.clone()), at(0));

        assert_eq!(state.calibration.status, CalibrationStatus::Running);
        assert_eq!(state.calibration.progress, Some(rejected));
        assert_eq!(state.calibration.completed_notes, BTreeSet::new());

        state.reduce_event(
            EngineEvent::CalibrationNoteCompleted(CalibrationNoteCompletion {
                midi_note: 36,
                completed_notes: 1,
                total_notes: CALIBRATION_NOTE_COUNT,
                accepted_samples: 5,
            }),
            at(1),
        );
        assert!(state.calibration.completed_notes.contains(&36));

        let template_path = PathBuf::from("/tmp/pss-f30-templates.json");
        state.reduce_event(
            EngineEvent::CalibrationCompleted {
                note_count: 37,
                sample_count: 185,
                template_path: template_path.clone(),
            },
            at(2),
        );
        assert_eq!(state.calibration.status, CalibrationStatus::Completed);
        assert_eq!(
            state.calibration.completion,
            Some(CalibrationCompletion {
                note_count: 37,
                sample_count: 185,
                template_path,
            })
        );

        state.reduce_event(EngineEvent::CalibrationCancelled, at(3));
        assert_eq!(state.calibration.status, CalibrationStatus::Cancelled);
        assert_eq!(state.calibration.completion, None);

        state.reduce_event(
            EngineEvent::CalibrationError {
                message: "template save failed".to_owned(),
            },
            at(4),
        );
        assert_eq!(state.calibration.status, CalibrationStatus::Error);
        assert_eq!(
            state.error_banner,
            Some(ErrorBanner {
                kind: ErrorKind::Calibration,
                message: "template save failed".to_owned(),
            })
        );
    }

    #[test]
    fn runtime_and_device_errors_set_banners_and_device_state_is_reduced() {
        let mut config = AppConfig::default();
        config.audio.device = "hw:old,0".to_owned();
        let mut state = AppState::new(config);

        state.reduce_event(
            EngineEvent::AudioDevicesEnumerated {
                devices: vec![AudioInputDevice {
                    id: "hw:1,0".to_owned(),
                    label: "Microphone".to_owned(),
                }],
                selected_device: "hw:1,0".to_owned(),
            },
            at(0),
        );
        assert_eq!(state.audio_devices.len(), 1);
        assert_eq!(state.selected_audio_device, "hw:1,0");
        assert_eq!(state.config.audio.device, "hw:1,0");

        state.reduce_event(
            EngineEvent::AudioDeviceUnavailable {
                device_id: "hw:1,0".to_owned(),
                message: "device is busy".to_owned(),
            },
            at(1),
        );
        assert!(!state.audio_device_open);
        assert_eq!(
            state.error_banner,
            Some(ErrorBanner {
                kind: ErrorKind::AudioDevice,
                message: "device is busy".to_owned(),
            })
        );
        state.reduce_event(
            EngineEvent::AudioDeviceOpened {
                device_id: "hw:1,0".to_owned(),
            },
            at(2),
        );
        assert!(state.audio_device_open);
        assert_eq!(state.error_banner, None);

        state.reduce_event(
            EngineEvent::AudioDeviceEnumerationFailed {
                message: "ALSA unavailable".to_owned(),
            },
            at(3),
        );
        assert_eq!(
            state.error_banner.as_ref().map(|banner| banner.kind),
            Some(ErrorKind::AudioDeviceEnumeration)
        );

        state.reduce_event(
            EngineEvent::Error {
                message: "MIDI output disconnected".to_owned(),
            },
            at(4),
        );
        assert_eq!(
            state.error_banner,
            Some(ErrorBanner {
                kind: ErrorKind::Runtime,
                message: "MIDI output disconnected".to_owned(),
            })
        );
        state.clear_error_banner();
        assert_eq!(state.error_banner, None);
    }

    #[test]
    fn selecting_same_open_device_preserves_open_state_until_device_changes() {
        let mut state = AppState::default();
        state.reduce_event_at(
            EngineEvent::AudioDeviceOpened {
                device_id: "pipewire".to_owned(),
            },
            at(0),
            wall_at(0),
        );
        assert!(state.audio_device_open);

        state.reduce_event_at(
            EngineEvent::AudioDeviceSelected {
                device_id: "pipewire".to_owned(),
            },
            at(1),
            wall_at(1),
        );
        assert!(state.audio_device_open);

        state.reduce_event_at(
            EngineEvent::AudioDeviceSelected {
                device_id: "hw:1,0".to_owned(),
            },
            at(2),
            wall_at(2),
        );
        assert_eq!(state.selected_audio_device, "hw:1,0");
        assert!(!state.audio_device_open);
    }

    #[test]
    fn recent_event_history_is_bounded_to_one_hundred_entries() {
        let mut state = AppState::default();
        for index in 0..105 {
            state.reduce_event(
                EngineEvent::Error {
                    message: format!("failure {index}"),
                },
                at(index),
            );
        }

        assert_eq!(state.recent_events.len(), RECENT_EVENT_LIMIT);
        assert!(matches!(
            &state.recent_events.front().unwrap().kind,
            RecentEventKind::Error { message } if message == "failure 5"
        ));
        assert!(matches!(
            &state.recent_events.back().unwrap().kind,
            RecentEventKind::Error { message } if message == "failure 104"
        ));
    }

    #[test]
    fn audio_levels_are_coalesced_but_semantic_events_are_immediate() {
        let mut state = AppState::default();
        state.reduce_event(
            EngineEvent::AudioLevel {
                rms_dbfs: -30.0,
                peak_dbfs: -6.0,
            },
            at(0),
        );
        state.reduce_event(
            EngineEvent::AudioLevel {
                rms_dbfs: -20.0,
                peak_dbfs: -3.0,
            },
            at(20),
        );
        assert_eq!(
            state.audio_levels,
            Some(AudioLevels {
                rms_dbfs: -30.0,
                peak_dbfs: -6.0,
            })
        );
        assert!(state.recent_events.is_empty());

        state.reduce_event(EngineEvent::NoteOn { midi_note: 60 }, at(21));
        assert_eq!(state.active_note, Some(60));
        assert_eq!(state.recent_events.len(), 1);

        state.reduce_event(
            EngineEvent::AudioLevel {
                rms_dbfs: -18.0,
                peak_dbfs: -2.0,
            },
            at(40),
        );
        assert_eq!(
            state.audio_levels,
            Some(AudioLevels {
                rms_dbfs: -18.0,
                peak_dbfs: -2.0,
            })
        );
        assert_eq!(state.recent_events.len(), 1);

        state.reduce_event(EngineEvent::NoteOff { midi_note: 60 }, at(41));
        assert_eq!(state.active_note, None);
        assert_eq!(state.recent_events.len(), 2);
    }
}
