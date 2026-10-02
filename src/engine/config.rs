//! GPUI-independent runtime configuration for the audio engine.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorMode {
    Yin,
    Spectral,
    Compare,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    /// ALSA capture PCM. `pipewire` uses PipeWire's default source.
    pub device: String,
    pub sample_rate: u32,
    pub hop: usize,
    pub onset_buffer: usize,
    pub silence_db: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: "pipewire".to_owned(),
            sample_rate: 48_000,
            hop: 128,
            onset_buffer: 1024,
            silence_db: -45.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpectralConfig {
    /// Delay after an onset before spectral feature capture begins.
    pub spectral_delay_ms: f32,
    /// Audio duration used for each spectral observation.
    pub spectral_window_ms: f32,
    pub fft_size: usize,
    pub spectral_min_score: f32,
    pub spectral_min_margin: f32,
}

impl Default for SpectralConfig {
    fn default() -> Self {
        Self {
            spectral_delay_ms: 8.0,
            spectral_window_ms: 30.0,
            fft_size: 2048,
            spectral_min_score: 0.75,
            spectral_min_margin: 0.03,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunConfig {
    pub audio: AudioConfig,
    pub detector: DetectorMode,
    pub pitch_buffer: usize,
    pub attack_ignore_ms: f32,
    pub decision_window_ms: f32,
    pub decision_extend_ms: f32,
    pub release_ms: f32,
    pub retrigger_ms: f32,
    pub onset_threshold: f32,
    pub vote_ratio: f32,
    pub initial_stable: usize,
    pub template_path: PathBuf,
    pub spectral: SpectralConfig,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            detector: DetectorMode::Yin,
            pitch_buffer: 2048,
            attack_ignore_ms: 10.0,
            decision_window_ms: 20.0,
            decision_extend_ms: 10.0,
            release_ms: 30.0,
            retrigger_ms: 60.0,
            onset_threshold: 0.30,
            vote_ratio: 0.60,
            initial_stable: 10,
            template_path: default_template_path(),
            spectral: SpectralConfig::default(),
        }
    }
}

pub fn default_template_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/pss2midi/pss-f30-templates.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_configuration_defaults_match_desktop_engine_defaults() {
        let config = RunConfig::default();

        assert_eq!(config.audio.device, "pipewire");
        assert_eq!(config.audio.sample_rate, 48_000);
        assert_eq!(config.audio.hop, 128);
        assert_eq!(config.audio.onset_buffer, 1024);
        assert_eq!(config.audio.silence_db, -45.0);
        assert_eq!(config.detector, DetectorMode::Yin);
        assert_eq!(config.pitch_buffer, 2048);
        assert_eq!(config.attack_ignore_ms, 10.0);
        assert_eq!(config.decision_window_ms, 20.0);
        assert_eq!(config.decision_extend_ms, 10.0);
        assert_eq!(config.release_ms, 30.0);
        assert_eq!(config.retrigger_ms, 60.0);
        assert_eq!(config.onset_threshold, 0.30);
        assert_eq!(config.vote_ratio, 0.60);
        assert_eq!(config.initial_stable, 10);
        assert_eq!(config.template_path, default_template_path());
        assert_eq!(config.spectral, SpectralConfig::default());
    }
}
