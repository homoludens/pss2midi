use alsa::{
    pcm::{Access, Format, HwParams, IO, PCM},
    Direction, ValueOr,
};
use anyhow::{Context, Result};

use crate::engine::config::AudioArgs;

pub fn open_capture(args: &AudioArgs) -> Result<(PCM, u32, usize)> {
    let pcm = PCM::new(&args.device, Direction::Capture, false)
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

/// Sequential fixed-size PCM frames, retaining partial ALSA reads between calls.
pub struct AudioFrameReader<'a> {
    pcm: &'a PCM,
    io: IO<'a, i16>,
    raw: Vec<i16>,
    raw_offset: usize,
    raw_len: usize,
    sample_index: u64,
}

impl<'a> AudioFrameReader<'a> {
    pub fn new(pcm: &'a PCM, hop: usize) -> Result<Self> {
        Ok(Self {
            pcm,
            io: pcm.io_i16()?,
            raw: vec![0; hop],
            raw_offset: 0,
            raw_len: 0,
            sample_index: 0,
        })
    }

    pub fn next_frame(&mut self, frame: &mut [f32]) -> Result<u64> {
        anyhow::ensure!(!frame.is_empty(), "audio frame cannot be empty");
        let frame_start = self.sample_index;
        let frame_len = frame.len();
        for sample in frame.iter_mut() {
            while self.raw_offset == self.raw_len {
                match self.io.readi(&mut self.raw) {
                    Ok(frames_read) => {
                        anyhow::ensure!(frames_read > 0, "ALSA capture returned an empty frame");
                        self.raw_offset = 0;
                        self.raw_len = frames_read;
                    }
                    Err(error) => {
                        eprintln!("ALSA capture error: {error}; trying recovery");
                        self.pcm.try_recover(error, false)?;
                        self.raw_offset = 0;
                        self.raw_len = 0;
                    }
                }
            }
            *sample = self.raw[self.raw_offset] as f32 / 32768.0;
            self.raw_offset += 1;
        }
        self.sample_index += frame_len as u64;
        Ok(frame_start)
    }
}
