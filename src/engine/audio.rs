use alsa::{
    pcm::{Access, Format, HwParams, IO, PCM},
    Direction, ValueOr,
};
use anyhow::{Context, Result};
use std::{
    io,
    time::{Duration, Instant},
};

use crate::engine::config::AudioConfig;

pub fn open_capture(args: &AudioConfig) -> Result<(PCM, u32, usize)> {
    open_capture_with_mode(args, false)
}

pub(crate) fn open_capture_for_worker(
    args: &AudioConfig,
) -> Result<(CaptureFrameReader, u32, usize)> {
    let (pcm, sample_rate, actual_period) = open_capture_with_mode(args, true)?;
    let reader = CaptureFrameReader::new(pcm, actual_period)?;
    Ok((reader, sample_rate, actual_period))
}

fn open_capture_with_mode(args: &AudioConfig, nonblocking: bool) -> Result<(PCM, u32, usize)> {
    let pcm = PCM::new(&args.device, Direction::Capture, nonblocking)
        .with_context(|| format!("Cannot open ALSA capture PCM '{}'", args.device))?;

    let (actual_rate, actual_period);
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_channels(1)?;
        actual_rate = hwp.set_rate_near(args.sample_rate, ValueOr::Nearest)?;
        actual_period = hwp.set_period_size_near(args.hop as i64, ValueOr::Nearest)?;
        let _ = hwp.set_buffer_size_near(actual_period * 4);
        pcm.hw_params(&hwp)?;
    }

    pcm.prepare()?;
    Ok((pcm, actual_rate, actual_period as usize))
}

/// Worker capture reader that retains partial frames across bounded polling
/// calls. The PCM is nonblocking, so no individual ALSA read can hold up
/// command processing indefinitely.
pub(crate) struct CaptureFrameReader {
    pcm: PCM,
    raw: Vec<i16>,
    raw_offset: usize,
    raw_len: usize,
    pending_frame: Vec<f32>,
    pending_fill: usize,
    sample_index: u64,
}

pub(crate) trait AudioFrameSource: Send {
    fn try_next_frame(&mut self, frame: &mut [f32], timeout: Duration) -> Result<Option<u64>>;
}

impl CaptureFrameReader {
    fn new(pcm: PCM, hop: usize) -> Result<Self> {
        anyhow::ensure!(hop > 0, "audio hop cannot be empty");
        Ok(Self {
            pcm,
            raw: vec![0; hop],
            raw_offset: 0,
            raw_len: 0,
            pending_frame: Vec::new(),
            pending_fill: 0,
            sample_index: 0,
        })
    }

    pub fn try_next_frame(&mut self, frame: &mut [f32], timeout: Duration) -> Result<Option<u64>> {
        anyhow::ensure!(!frame.is_empty(), "audio frame cannot be empty");
        anyhow::ensure!(
            self.pending_fill == 0 || self.pending_frame.len() == frame.len(),
            "audio frame size changed while a partial frame was pending"
        );
        if self.pending_frame.len() != frame.len() {
            self.pending_frame.resize(frame.len(), 0.0);
        }

        let frame_start = self.sample_index;
        let deadline = Instant::now() + timeout;
        loop {
            while self.raw_offset < self.raw_len && self.pending_fill < frame.len() {
                self.pending_frame[self.pending_fill] = self.raw[self.raw_offset] as f32 / 32768.0;
                self.raw_offset += 1;
                self.pending_fill += 1;
            }

            if self.pending_fill == frame.len() {
                frame.copy_from_slice(&self.pending_frame);
                self.pending_fill = 0;
                self.sample_index += frame.len() as u64;
                return Ok(Some(frame_start));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let timeout_ms = remaining.as_millis().max(1).min(u32::MAX as u128) as u32;
            if !self.pcm.wait(Some(timeout_ms))? {
                return Ok(None);
            }

            let read_result = {
                let io: IO<'_, i16> = self.pcm.io_i16()?;
                io.readi(&mut self.raw)
            };
            match read_result {
                Ok(frames_read) => {
                    anyhow::ensure!(frames_read > 0, "ALSA capture returned an empty frame");
                    self.raw_offset = 0;
                    self.raw_len = frames_read;
                }
                Err(error)
                    if io::Error::from_raw_os_error(error.errno()).kind()
                        == io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(error) => {
                    self.pcm.try_recover(error, false)?;
                    self.raw_offset = 0;
                    self.raw_len = 0;
                    self.pending_fill = 0;
                    return Ok(None);
                }
            }
        }
    }
}

impl AudioFrameSource for CaptureFrameReader {
    fn try_next_frame(&mut self, frame: &mut [f32], timeout: Duration) -> Result<Option<u64>> {
        CaptureFrameReader::try_next_frame(self, frame, timeout)
    }
}
