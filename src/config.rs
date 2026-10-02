use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "pss2midi", about = "Low-latency Yamaha PSS-F30 audio to MIDI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[command(flatten)]
    pub run: RunArgs,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Capture spectral examples for all 37 PSS-F30 notes.
    Calibrate(CalibrateArgs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DetectorMode {
    Yin,
    Spectral,
    Compare,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct AudioArgs {
    /// ALSA capture PCM. "pipewire" uses PipeWire's default source.
    #[arg(long, default_value = "pipewire")]
    pub device: String,

    #[arg(long, default_value_t = 48_000)]
    pub sample_rate: u32,

    /// Requested ALSA/aubio hop size.
    #[arg(long, default_value_t = 128)]
    pub hop: usize,

    /// aubio onset analysis window.
    #[arg(long, default_value_t = 1024)]
    pub onset_buffer: usize,

    #[arg(long, default_value_t = -45.0, allow_hyphen_values = true)]
    pub silence_db: f32,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct RunArgs {
    #[command(flatten)]
    pub audio: AudioArgs,

    /// Detector to use. YIN is the existing aubio pitch path.
    #[arg(long, value_enum, default_value_t = DetectorMode::Yin)]
    pub detector: DetectorMode,

    /// aubio YIN pitch window.
    #[arg(long, default_value_t = 2048)]
    pub pitch_buffer: usize,

    /// Ignore pitch estimates for this long after an onset.
    #[arg(long, default_value_t = 10.0)]
    pub attack_ignore_ms: f32,

    /// Collect pitch votes for this long after attack-ignore.
    #[arg(long, default_value_t = 20.0)]
    pub decision_window_ms: f32,

    /// If decision is ambiguous, extend once by this amount.
    #[arg(long, default_value_t = 10.0)]
    pub decision_extend_ms: f32,

    /// Silence required before NOTE OFF.
    #[arg(long, default_value_t = 30.0)]
    pub release_ms: f32,

    /// Minimum time between repeated same-note attacks.
    #[arg(long, default_value_t = 60.0)]
    pub retrigger_ms: f32,

    /// aubio onset peak threshold.
    #[arg(long, default_value_t = 0.30)]
    pub onset_threshold: f32,

    /// Fraction of valid YIN pitch votes required for winner.
    #[arg(long, default_value_t = 0.60)]
    pub vote_ratio: f32,

    /// Stable frames required for YIN initial fallback acquisition.
    #[arg(long, default_value_t = 10)]
    pub initial_stable: usize,

    /// Saved templates; defaults to ~/.config/pss2midi/pss-f30-templates.json.
    #[arg(long)]
    pub templates: Option<PathBuf>,

    #[command(flatten)]
    pub spectral: SpectralArgs,

    #[arg(long)]
    pub debug: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct SpectralArgs {
    /// Ignore this many milliseconds after the detected onset.
    #[arg(long, default_value_t = 8.0)]
    pub spectral_delay_ms: f32,

    /// Audio duration used for each spectral observation.
    #[arg(long, default_value_t = 30.0)]
    pub spectral_window_ms: f32,

    /// FFT size; must be a power of two and at least the window length.
    #[arg(long, default_value_t = 2048)]
    pub fft_size: usize,

    #[arg(long, default_value_t = 0.75)]
    pub spectral_min_score: f32,

    #[arg(long, default_value_t = 0.03)]
    pub spectral_min_margin: f32,
}

#[derive(ClapArgs, Debug)]
pub struct CalibrateArgs {
    #[command(flatten)]
    pub audio: AudioArgs,

    /// Destination JSON file (defaults to ~/.config/pss2midi/pss-f30-templates.json).
    #[arg(long)]
    pub output: Option<PathBuf>,

    #[arg(long, default_value_t = 5)]
    pub samples_per_note: usize,

    #[command(flatten)]
    pub spectral: SpectralArgs,
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
    fn existing_run_command_defaults_to_yin() {
        let cli = Cli::try_parse_from(["pss2midi", "--device", "pipewire", "--silence-db", "-45"])
            .unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.run.detector, DetectorMode::Yin);
        assert_eq!(cli.run.audio.device, "pipewire");
        assert_eq!(cli.run.audio.silence_db, -45.0);
    }

    #[test]
    fn detector_modes_are_selectable() {
        for (argument, expected) in [
            ("yin", DetectorMode::Yin),
            ("spectral", DetectorMode::Spectral),
            ("compare", DetectorMode::Compare),
        ] {
            let cli = Cli::try_parse_from(["pss2midi", "--detector", argument]).unwrap();
            assert_eq!(cli.run.detector, expected);
        }
    }

    #[test]
    fn calibration_subcommand_accepts_capture_options() {
        let cli =
            Cli::try_parse_from(["pss2midi", "calibrate", "--samples-per-note", "3"]).unwrap();
        let Some(Commands::Calibrate(args)) = cli.command else {
            panic!("calibrate subcommand was not parsed");
        };
        assert_eq!(args.samples_per_note, 3);
    }
}
