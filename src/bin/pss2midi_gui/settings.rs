//! Settings-page projection and validated edits for persisted application settings.

use std::{fmt::Display, path::PathBuf, str::FromStr};

use pss2midi::{
    engine::{app_config::AppConfig, config::DetectorMode},
    ui::state::{AppState, MidiOutputStatus},
};

use crate::live::{audio_device_choices, AudioDeviceChoice};

pub(super) const ADVANCED_SETTINGS_EXPANDED_BY_DEFAULT: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingField {
    SampleRate,
    TemplatePath,
    SpectralDelay,
    SpectralWindow,
    SpectralMinimumScore,
    SpectralMinimumMargin,
    HopSize,
    OnsetBufferSize,
    SilenceDb,
    PitchBufferSize,
    AttackIgnoreMs,
    DecisionWindowMs,
    DecisionExtendMs,
    ReleaseMs,
    RetriggerMs,
    OnsetThreshold,
    VoteRatio,
    InitialStableFrames,
    FftSize,
}

impl SettingField {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::SampleRate => "sample-rate",
            Self::TemplatePath => "template-path",
            Self::SpectralDelay => "spectral-delay",
            Self::SpectralWindow => "spectral-window",
            Self::SpectralMinimumScore => "spectral-minimum-score",
            Self::SpectralMinimumMargin => "spectral-minimum-margin",
            Self::HopSize => "hop-size",
            Self::OnsetBufferSize => "onset-buffer-size",
            Self::SilenceDb => "silence-db",
            Self::PitchBufferSize => "pitch-buffer-size",
            Self::AttackIgnoreMs => "attack-ignore-ms",
            Self::DecisionWindowMs => "decision-window-ms",
            Self::DecisionExtendMs => "decision-extend-ms",
            Self::ReleaseMs => "release-ms",
            Self::RetriggerMs => "retrigger-ms",
            Self::OnsetThreshold => "onset-threshold",
            Self::VoteRatio => "vote-ratio",
            Self::InitialStableFrames => "initial-stable-frames",
            Self::FftSize => "fft-size",
        }
    }

    pub(super) fn value(self, config: &AppConfig) -> String {
        match self {
            Self::SampleRate => config.audio.sample_rate.to_string(),
            Self::TemplatePath => config.template_path.to_string_lossy().into_owned(),
            Self::SpectralDelay => config.spectral.delay_ms.to_string(),
            Self::SpectralWindow => config.spectral.window_ms.to_string(),
            Self::SpectralMinimumScore => config.spectral.minimum_score.to_string(),
            Self::SpectralMinimumMargin => config.spectral.minimum_margin.to_string(),
            Self::HopSize => config.advanced.hop_size.to_string(),
            Self::OnsetBufferSize => config.advanced.onset_buffer_size.to_string(),
            Self::SilenceDb => config.advanced.silence_db.to_string(),
            Self::PitchBufferSize => config.advanced.pitch_buffer_size.to_string(),
            Self::AttackIgnoreMs => config.advanced.attack_ignore_ms.to_string(),
            Self::DecisionWindowMs => config.advanced.decision_window_ms.to_string(),
            Self::DecisionExtendMs => config.advanced.decision_extend_ms.to_string(),
            Self::ReleaseMs => config.advanced.release_ms.to_string(),
            Self::RetriggerMs => config.advanced.retrigger_ms.to_string(),
            Self::OnsetThreshold => config.advanced.onset_threshold.to_string(),
            Self::VoteRatio => config.advanced.vote_ratio.to_string(),
            Self::InitialStableFrames => config.advanced.initial_stable_frames.to_string(),
            Self::FftSize => config.advanced.fft_size.to_string(),
        }
    }
}

/// Values and engine-backed status shown by the Settings view.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SettingsViewModel {
    pub(super) config: AppConfig,
    pub(super) audio_choices: Vec<AudioDeviceChoice>,
    pub(super) selected_audio_device: String,
    pub(super) detector_mode: DetectorMode,
    pub(super) midi_output_status: MidiOutputStatus,
    pub(super) advanced_expanded: bool,
}

impl SettingsViewModel {
    pub(super) fn from_state(state: &AppState, advanced_expanded: bool) -> Self {
        Self {
            config: state.config.clone(),
            audio_choices: audio_device_choices(&state.audio_devices, &state.selected_audio_device),
            selected_audio_device: state.selected_audio_device.clone(),
            detector_mode: state.config.detector_mode,
            midi_output_status: state.midi_output_status.clone(),
            advanced_expanded,
        }
    }
}

/// Apply a textual edit to a candidate configuration and enforce the same
/// validation rules used by the atomic persistence API before accepting it.
pub(super) fn apply_text_edit(
    current: &AppConfig,
    field: SettingField,
    text: &str,
) -> Result<AppConfig, String> {
    let mut updated = current.clone();
    match field {
        SettingField::SampleRate => {
            updated.audio.sample_rate = parse_value(text, "sample rate")?;
        }
        SettingField::TemplatePath => updated.template_path = PathBuf::from(text),
        SettingField::SpectralDelay => {
            updated.spectral.delay_ms = parse_value(text, "spectral delay")?;
        }
        SettingField::SpectralWindow => {
            updated.spectral.window_ms = parse_value(text, "spectral window")?;
        }
        SettingField::SpectralMinimumScore => {
            updated.spectral.minimum_score = parse_value(text, "minimum score")?;
        }
        SettingField::SpectralMinimumMargin => {
            updated.spectral.minimum_margin = parse_value(text, "minimum margin")?;
        }
        SettingField::HopSize => updated.advanced.hop_size = parse_value(text, "hop size")?,
        SettingField::OnsetBufferSize => {
            updated.advanced.onset_buffer_size = parse_value(text, "onset buffer size")?;
        }
        SettingField::SilenceDb => {
            updated.advanced.silence_db = parse_value(text, "silence threshold")?;
        }
        SettingField::PitchBufferSize => {
            updated.advanced.pitch_buffer_size = parse_value(text, "YIN pitch buffer size")?;
        }
        SettingField::AttackIgnoreMs => {
            updated.advanced.attack_ignore_ms = parse_value(text, "YIN attack ignore")?;
        }
        SettingField::DecisionWindowMs => {
            updated.advanced.decision_window_ms = parse_value(text, "YIN decision window")?;
        }
        SettingField::DecisionExtendMs => {
            updated.advanced.decision_extend_ms = parse_value(text, "YIN decision extension")?;
        }
        SettingField::ReleaseMs => {
            updated.advanced.release_ms = parse_value(text, "release time")?;
        }
        SettingField::RetriggerMs => {
            updated.advanced.retrigger_ms = parse_value(text, "retrigger time")?;
        }
        SettingField::OnsetThreshold => {
            updated.advanced.onset_threshold = parse_value(text, "onset threshold")?;
        }
        SettingField::VoteRatio => {
            updated.advanced.vote_ratio = parse_value(text, "YIN vote ratio")?;
        }
        SettingField::InitialStableFrames => {
            updated.advanced.initial_stable_frames = parse_value(text, "stable frame count")?;
        }
        SettingField::FftSize => updated.advanced.fft_size = parse_value(text, "FFT size")?,
    }

    updated.validate().map_err(|error| error.to_string())?;
    Ok(updated)
}

fn parse_value<T>(text: &str, label: &str) -> Result<T, String>
where
    T: FromStr,
    T::Err: Display,
{
    text.trim()
        .parse()
        .map_err(|error| format!("Invalid {label}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use pss2midi::{
        engine::{
            app_config::{AdvancedSettings, AppConfig},
            config::DetectorMode,
            AudioInputDevice,
        },
        ui::state::{AppState, MidiOutputStatus},
    };

    use super::{
        apply_text_edit, SettingField, SettingsViewModel, ADVANCED_SETTINGS_EXPANDED_BY_DEFAULT,
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system clock is before the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "pss2midi-settings-test-{}-{timestamp}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temporary settings directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn settings_view_model_maps_config_devices_and_actual_midi_status() {
        let mut config = AppConfig::default();
        config.audio.device = "hw:2,0".to_owned();
        config.audio.sample_rate = 44_100;
        config.detector_mode = DetectorMode::Compare;
        config.template_path = PathBuf::from("/tmp/custom-piano-templates.json");
        config.spectral.delay_ms = 12.5;
        config.advanced.fft_size = 4096;
        let mut state = AppState::new(config.clone());
        state.audio_devices = vec![AudioInputDevice {
            id: "hw:2,0".to_owned(),
            label: "USB keyboard".to_owned(),
        }];
        state.midi_output_status = MidiOutputStatus::Available {
            name: "PSS-F30 Audio MIDI".to_owned(),
        };

        let view = SettingsViewModel::from_state(&state, false);

        assert_eq!(view.config, config);
        assert_eq!(view.selected_audio_device, "hw:2,0");
        assert_eq!(view.audio_choices.len(), 2);
        assert!(view.audio_choices.iter().any(|choice| {
            choice.id == "hw:2,0" && choice.label == "USB keyboard" && choice.selected
        }));
        assert_eq!(view.detector_mode, DetectorMode::Compare);
        assert_eq!(
            view.midi_output_status,
            MidiOutputStatus::Available {
                name: "PSS-F30 Audio MIDI".to_owned()
            }
        );
    }

    #[test]
    fn advanced_controls_start_collapsed() {
        let view = SettingsViewModel::from_state(
            &AppState::default(),
            ADVANCED_SETTINGS_EXPANDED_BY_DEFAULT,
        );

        assert!(!view.advanced_expanded);
    }

    #[test]
    fn edited_settings_save_and_reload_from_a_temporary_config_path() {
        let directory = TestDirectory::new();
        let path = directory.0.join("config.json");
        let config = AppConfig::default();
        let mut edited = apply_text_edit(&config, SettingField::SampleRate, "44100").unwrap();
        edited = apply_text_edit(
            &edited,
            SettingField::TemplatePath,
            "/tmp/settings-test-templates.json",
        )
        .unwrap();
        edited = apply_text_edit(&edited, SettingField::SpectralDelay, "11.5").unwrap();
        edited = apply_text_edit(&edited, SettingField::ReleaseMs, "42").unwrap();
        edited = apply_text_edit(&edited, SettingField::FftSize, "4096").unwrap();
        edited.detector_mode = DetectorMode::Spectral;
        edited.validate().unwrap();

        pss2midi::engine::save_config_to(&edited, &path).unwrap();
        let reloaded = pss2midi::engine::load_config_from(&path).unwrap();

        assert_eq!(reloaded, edited);
        assert_eq!(reloaded.audio.sample_rate, 44_100);
        assert_eq!(
            reloaded.template_path,
            PathBuf::from("/tmp/settings-test-templates.json")
        );
        assert_eq!(reloaded.spectral.delay_ms, 11.5);
        assert_eq!(reloaded.advanced.release_ms, 42.0);
        assert_eq!(reloaded.advanced.fft_size, 4096);
        assert_eq!(reloaded.detector_mode, DetectorMode::Spectral);
    }

    #[test]
    fn invalid_edits_are_rejected_without_changing_the_current_config() {
        let config = AppConfig {
            detector_mode: DetectorMode::Spectral,
            advanced: AdvancedSettings {
                fft_size: 2048,
                ..AdvancedSettings::default()
            },
            ..AppConfig::default()
        };

        assert!(apply_text_edit(&config, SettingField::SampleRate, "0").is_err());
        assert!(apply_text_edit(&config, SettingField::SpectralMinimumScore, "1.5").is_err());
        assert!(apply_text_edit(&config, SettingField::SampleRate, "not a number").is_err());
        assert_eq!(
            config,
            AppConfig {
                detector_mode: DetectorMode::Spectral,
                advanced: AdvancedSettings {
                    fft_size: 2048,
                    ..AdvancedSettings::default()
                },
                ..AppConfig::default()
            }
        );
    }
}
