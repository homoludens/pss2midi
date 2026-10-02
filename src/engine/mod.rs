//! Audio capture, DSP, MIDI, calibration, and template functionality.
//!
//! This module is shared by the desktop application and library consumers. It
//! intentionally contains no GPUI types or dependencies.

pub mod app_config;
pub mod audio;
mod audio_device;
pub mod calibration;
pub mod config;
pub mod detector;
pub mod features;
pub mod midi;
pub mod note;
pub mod onset;
mod persistence;
pub mod templates;
mod worker;

pub use app_config::{
    default_config_path, load_config, load_config_from, load_config_or_default,
    load_config_or_default_from, save_config, save_config_to, AdvancedSettings, AppConfig,
    AudioSettings, ConfigError, ConfigLoadResult, ConfigValidationError, SpectralSettings,
};
pub use audio_device::{AudioInputDevice, PIPEWIRE_DEFAULT_DEVICE_ID};
pub use detector::{DetectorOutcome, NoteDecision, SpectralResult, YinDecision, YinResult};
pub use worker::{EngineCommand, EngineEvent, EngineState, PssEngine, TemplateStatus};
