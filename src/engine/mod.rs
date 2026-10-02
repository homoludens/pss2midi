//! Audio capture, DSP, MIDI, calibration, and template functionality.
//!
//! This module is shared by the command-line application and the desktop GUI
//! library consumer. It intentionally contains no GPUI types or dependencies.

pub mod audio;
pub mod calibration;
pub mod config;
pub mod detector;
pub mod features;
pub mod midi;
pub mod note;
pub mod onset;
pub mod templates;
