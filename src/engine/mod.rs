//! Audio capture, DSP, MIDI, calibration, and template functionality.
//!
//! This module is shared by the command-line application and the desktop GUI
//! library consumer. It intentionally contains no GPUI types or dependencies.

pub mod audio;
mod audio_device;
pub mod calibration;
pub mod config;
pub mod detector;
pub mod features;
pub mod midi;
pub mod note;
pub mod onset;
pub mod templates;
mod worker;

pub use audio_device::AudioInputDevice;
pub use detector::{DetectorOutcome, NoteDecision, SpectralResult, YinDecision, YinResult};
pub use worker::{EngineCommand, EngineEvent, EngineState, PssEngine};
