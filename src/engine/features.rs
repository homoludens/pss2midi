//! Spectral feature extraction and indexed audio-window helpers.

use anyhow::{ensure, Result};
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::{collections::VecDeque, sync::Arc};

const MIN_FEATURE_HZ: f32 = 50.0;
const MAX_FEATURE_HZ: f32 = 8_000.0;
const LOG_SCALE: f32 = 10.0;

pub struct FeatureExtractor {
    sample_rate: u32,
    window_ms: f32,
    fft_size: usize,
    window: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    fft: Arc<dyn Fft<f32>>,
    first_bin: usize,
    end_bin: usize,
}

impl FeatureExtractor {
    pub fn new(sample_rate: u32, window_ms: f32, fft_size: usize) -> Result<Self> {
        ensure!(sample_rate > 0, "sample rate must be greater than zero");
        ensure!(
            fft_size.is_power_of_two(),
            "FFT size must be a power of two"
        );
        ensure!(
            window_ms.is_finite() && window_ms > 0.0,
            "spectral window must be positive"
        );
        let window_len = (sample_rate as f32 * window_ms / 1000.0).round() as usize;
        ensure!(window_len > 1, "spectral window is too short");
        ensure!(
            window_len <= fft_size,
            "FFT size must be at least the spectral window length"
        );

        let window = (0..window_len)
            .map(|index| {
                0.5 - 0.5
                    * (2.0 * std::f32::consts::PI * index as f32 / (window_len - 1) as f32).cos()
            })
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let nyquist = sample_rate as f32 / 2.0;
        let first_bin =
            ((MIN_FEATURE_HZ * fft_size as f32 / sample_rate as f32).ceil() as usize).max(1);
        let end_bin = ((MAX_FEATURE_HZ.min(nyquist) * fft_size as f32 / sample_rate as f32).floor()
            as usize)
            .min(fft_size / 2);
        ensure!(
            end_bin > first_bin,
            "sample rate leaves no usable spectral bins"
        );

        Ok(Self {
            sample_rate,
            window_ms,
            fft_size,
            window,
            spectrum: vec![Complex::new(0.0, 0.0); fft_size],
            fft,
            first_bin,
            end_bin,
        })
    }

    pub fn window_samples(&self) -> usize {
        self.window.len()
    }

    pub fn feature_len(&self) -> usize {
        self.end_bin - self.first_bin + 1
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn window_ms(&self) -> f32 {
        self.window_ms
    }

    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    pub fn extract(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        ensure!(
            samples.len() == self.window.len(),
            "spectral sample window has the wrong length"
        );
        self.spectrum.fill(Complex::new(0.0, 0.0));
        for ((slot, sample), weight) in self
            .spectrum
            .iter_mut()
            .zip(samples.iter())
            .zip(self.window.iter())
        {
            slot.re = sample * weight;
        }
        self.fft.process(&mut self.spectrum);

        let mut features = self.spectrum[self.first_bin..=self.end_bin]
            .iter()
            .map(|bin| (1.0 + LOG_SCALE * bin.norm()).ln())
            .collect::<Vec<_>>();
        let norm = features
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        ensure!(
            norm.is_finite() && norm > 1e-12,
            "audio window has no usable spectral energy"
        );
        features.iter_mut().for_each(|value| *value /= norm);
        Ok(features)
    }
}

#[derive(Debug)]
pub struct SampleCapture {
    start: u64,
    end: u64,
    samples: Vec<f32>,
}

impl SampleCapture {
    pub fn new(start: u64, length: usize) -> Self {
        Self {
            start,
            end: start + length as u64,
            samples: Vec::with_capacity(length),
        }
    }

    pub fn push_frame(&mut self, frame_start: u64, frame: &[f32]) {
        let frame_end = frame_start + frame.len() as u64;
        let copy_start = self.start.max(frame_start);
        let copy_end = self.end.min(frame_end);
        if copy_start < copy_end {
            let from = (copy_start - frame_start) as usize;
            let to = (copy_end - frame_start) as usize;
            for (offset, sample) in frame[from..to].iter().enumerate() {
                self.push_sample(copy_start + offset as u64, *sample);
            }
        }
    }

    pub fn push_sample(&mut self, index: u64, sample: f32) {
        let expected = self.start + self.samples.len() as u64;
        if index == expected && index < self.end {
            self.samples.push(sample);
        }
    }

    pub fn is_complete(&self) -> bool {
        self.samples.len() == (self.end - self.start) as usize
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }
}

/// Small rolling history so onset detections can include samples already
/// present in the analysis frame when the delayed spectral window is formed.
pub struct SampleHistory {
    start_index: u64,
    samples: VecDeque<f32>,
    capacity: usize,
}

impl SampleHistory {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            start_index: 0,
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push_frame(&mut self, frame_start: u64, frame: &[f32]) {
        if self.samples.is_empty() {
            self.start_index = frame_start;
        }
        for (offset, sample) in frame.iter().enumerate() {
            let index = frame_start + offset as u64;
            if self.samples.is_empty() {
                self.start_index = index;
            }
            while self.samples.len() >= self.capacity {
                self.samples.pop_front();
                self.start_index += 1;
            }
            self.samples.push_back(*sample);
        }
    }

    pub fn copy_available_to(&self, capture: &mut SampleCapture) {
        for (offset, sample) in self.samples.iter().enumerate() {
            capture.push_sample(self.start_index + offset as u64, *sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_selects_sample_indexed_window_across_frames() {
        let mut capture = SampleCapture::new(3, 4);
        capture.push_frame(0, &[0., 1., 2., 3.]);
        capture.push_frame(4, &[4., 5., 6., 7.]);
        assert!(capture.is_complete());
        assert_eq!(capture.samples(), &[3., 4., 5., 6.]);
    }

    #[test]
    fn feature_extractor_rejects_invalid_fft_size() {
        assert!(FeatureExtractor::new(48_000, 30.0, 1000).is_err());
    }

    #[test]
    fn sample_history_retains_recent_and_current_audio_for_capture() {
        let mut history = SampleHistory::new(5);
        history.push_frame(0, &[0., 1., 2., 3.]);
        history.push_frame(4, &[4., 5., 6.]);
        let mut capture = SampleCapture::new(2, 4);
        history.copy_available_to(&mut capture);
        assert!(capture.is_complete());
        assert_eq!(capture.samples(), &[2., 3., 4., 5.]);
    }
}
