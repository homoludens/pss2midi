use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "pss2midi", about = "Low-latency Yamaha PSS-F30 audio to MIDI")]
pub struct Args {
    /// ALSA capture PCM. "pipewire" uses PipeWire's default source.
    #[arg(long, default_value = "pipewire")]
    pub device: String,

    #[arg(long, default_value_t = 48_000)]
    pub sample_rate: u32,

    /// Requested ALSA/aubio hop size.
    #[arg(long, default_value_t = 128)]
    pub hop: usize,

    /// aubio YIN pitch window.
    #[arg(long, default_value_t = 2048)]
    pub pitch_buffer: usize,

    /// aubio onset analysis window.
    #[arg(long, default_value_t = 1024)]
    pub onset_buffer: usize,

    #[arg(long, default_value_t = -45.0, allow_hyphen_values = true)]
    pub silence_db: f32,

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

    /// Fraction of valid pitch votes required for winner.
    #[arg(long, default_value_t = 0.60)]
    pub vote_ratio: f32,

    /// Stable pitch frames required when no MIDI note is active.
    #[arg(long, default_value_t = 10)]
    pub initial_stable: usize,

    #[arg(long)]
    pub debug: bool,
}
