use alsa::{
    pcm::{Access, Format, HwParams, PCM},
    Direction, ValueOr,
};
use anyhow::{Context, Result};

use crate::config::Args;

pub fn open_capture(args: &Args) -> Result<(PCM, u32, usize)> {
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
