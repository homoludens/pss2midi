use anyhow::Result;
use aubio::{Onset, OnsetMode};

use crate::engine::config::AudioConfig;

pub struct OnsetDetector {
    onset: Onset,
}

impl OnsetDetector {
    pub fn new(
        args: &AudioConfig,
        onset_buffer: usize,
        threshold: f32,
        min_interval_ms: f32,
        sample_rate: u32,
        hop: usize,
    ) -> Result<Self> {
        let onset = Onset::new(OnsetMode::Hfc, onset_buffer, hop, sample_rate)?
            .with_silence(args.silence_db)
            .with_threshold(threshold)
            .with_minioi_ms(min_interval_ms);
        Ok(Self { onset })
    }

    pub fn detect(&mut self, frame: &[f32]) -> Result<bool> {
        Ok(self.onset.do_result(frame)? > 0.0)
    }
}
