use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use alsa::{
    pcm::{Access, Format, HwParams, PCM},
    Direction, ValueOr,
};
use anyhow::{Context, Result};
use aubio::{
    Onset, OnsetMode,
    Pitch, PitchMode, PitchUnit,
};
use clap::Parser;
use midir::{
    os::unix::VirtualOutput,
    MidiOutput,
    MidiOutputConnection,
};


// ---------------------------------------------------------
// Yamaha PSS-F30
// ---------------------------------------------------------

const MIN_MIDI: i32 = 36; // C2
const MAX_MIDI: i32 = 72; // C5
const NOTE_COUNT: usize = (MAX_MIDI - MIN_MIDI + 1) as usize;
const FALLBACK_ATTACK_MARGIN_DB: f32 = 10.0;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F",
    "F#", "G", "G#", "A", "A#", "B",
];


// ---------------------------------------------------------
// CLI
// ---------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "pss2midi",
    about = "Low-latency Yamaha PSS-F30 audio to MIDI"
)]
struct Args {
    /// ALSA capture PCM.
    /// "pipewire" uses PipeWire's current default source.
    #[arg(long, default_value = "pipewire")]
    device: String,

    #[arg(long, default_value_t = 48_000)]
    sample_rate: u32,

    /// Requested ALSA/aubio hop size.
    #[arg(long, default_value_t = 128)]
    hop: usize,

    /// aubio YIN pitch window.
    #[arg(long, default_value_t = 2048)]
    pitch_buffer: usize,

    /// aubio onset analysis window.
    #[arg(long, default_value_t = 1024)]
    onset_buffer: usize,

    #[arg(long, default_value_t = -45.0, allow_hyphen_values = true)]
    silence_db: f32,

    /// Ignore pitch estimates for this long after an onset.
    #[arg(long, default_value_t = 10.0)]
    attack_ignore_ms: f32,

    /// Collect pitch votes for this long after attack-ignore.
    #[arg(long, default_value_t = 20.0)]
    decision_window_ms: f32,

    /// If decision is ambiguous, extend once by this amount.
    #[arg(long, default_value_t = 10.0)]
    decision_extend_ms: f32,

    /// Silence required before NOTE OFF.
    #[arg(long, default_value_t = 30.0)]
    release_ms: f32,

    /// Minimum time between repeated same-note attacks.
    #[arg(long, default_value_t = 60.0)]
    retrigger_ms: f32,

    /// aubio onset peak threshold.
    #[arg(long, default_value_t = 0.30)]
    onset_threshold: f32,

    /// Fraction of valid pitch votes required for winner.
    #[arg(long, default_value_t = 0.60)]
    vote_ratio: f32,

    /// Fallback stability when no MIDI note is active.
    #[arg(long, default_value_t = 3)]
    initial_stable: usize,

    #[arg(long)]
    debug: bool,
}


// ---------------------------------------------------------
// Helpers
// ---------------------------------------------------------

fn note_name(note: u8) -> String {
    let octave = note as i32 / 12 - 1;

    format!(
        "{}{}",
        NOTE_NAMES[note as usize % 12],
        octave
    )
}


fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }

    let power = samples
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        / samples.len() as f64;

    let rms = power.sqrt();

    if rms <= 1e-12 {
        -120.0
    } else {
        (20.0 * rms.log10()) as f32
    }
}


fn ms_to_hops(ms: f32, rate: u32, hop: usize) -> usize {
    let samples = ms * rate as f32 / 1000.0;

    ((samples / hop as f32).ceil() as usize).max(1)
}


fn quantize_pitch(midi: f32) -> Option<(u8, f32)> {
    if !midi.is_finite() || midi <= 0.0 {
        return None;
    }

    let rounded = midi.round() as i32;

    if !(MIN_MIDI..=MAX_MIDI).contains(&rounded) {
        return None;
    }

    let cents = (midi - rounded as f32) * 100.0;

    // Require pitch to be reasonably close to a chromatic key.
    if cents.abs() > 45.0 {
        return None;
    }

    Some((rounded as u8, cents))
}


// ---------------------------------------------------------
// MIDI
// ---------------------------------------------------------

struct Midi {
    conn: MidiOutputConnection,
}


impl Midi {
    fn new() -> Result<Self> {
        let output =
            MidiOutput::new("pss2midi")
                .context("Cannot create MIDI client")?;

        let conn = output
            .create_virtual("PSS-F30 Audio MIDI")
            .map_err(|error| {
                anyhow::anyhow!(
                    "Cannot create virtual MIDI output: {error}"
                )
            })?;

        Ok(Self { conn })
    }


    fn note_on(&mut self, note: u8) -> Result<()> {
        self.conn
            .send(&[0x90, note, 100])
            .context("MIDI NOTE ON failed")?;

        println!(
            "ON   {:4} MIDI={}",
            note_name(note),
            note
        );

        Ok(())
    }


    fn note_off(&mut self, note: u8) -> Result<()> {
        self.conn
            .send(&[0x80, note, 0])
            .context("MIDI NOTE OFF failed")?;

        println!(
            "OFF  {:4} MIDI={}",
            note_name(note),
            note
        );

        Ok(())
    }
}


// ---------------------------------------------------------
// Onset decision window
// ---------------------------------------------------------

struct PendingDecision {
    onset_at: Instant,

    ignore_left: usize,
    collect_left: usize,

    extension_hops: usize,
    extensions_left: usize,

    votes: [u16; NOTE_COUNT],
    total_votes: u16,
}


impl PendingDecision {
    fn new(
        onset_at: Instant,
        ignore_hops: usize,
        decision_hops: usize,
        extension_hops: usize,
    ) -> Self {
        Self {
            onset_at,

            ignore_left: ignore_hops,
            collect_left: decision_hops,

            extension_hops,
            extensions_left: 1,

            votes: [0; NOTE_COUNT],
            total_votes: 0,
        }
    }


    fn vote(&mut self, note: u8) {
        let index = note as i32 - MIN_MIDI;

        if index < 0 || index as usize >= NOTE_COUNT {
            return;
        }

        self.votes[index as usize] += 1;
        self.total_votes += 1;
    }


    fn winner(&self) -> Option<(u8, u16, f32)> {
        if self.total_votes == 0 {
            return None;
        }

        let (index, count) = self
            .votes
            .iter()
            .enumerate()
            .max_by_key(|(_, count)| *count)?;

        let ratio =
            *count as f32 / self.total_votes as f32;

        let note =
            (MIN_MIDI + index as i32) as u8;

        Some((note, *count, ratio))
    }
}


// ---------------------------------------------------------
// Detector
// ---------------------------------------------------------

struct Detector {
    pitch: Pitch,
    onset: Onset,

    current_note: Option<u8>,

    pending: Option<PendingDecision>,

    fallback_note: Option<u8>,
    fallback_count: usize,

    silence_count: usize,

    attack_ignore_hops: usize,
    decision_hops: usize,
    decision_extension_hops: usize,
    release_hops: usize,

    initial_stable: usize,

    vote_ratio: f32,

    retrigger_time: Duration,
    last_trigger: Option<Instant>,

    silence_db: f32,

    debug: bool,
    started_at: Instant,
}


impl Detector {
    fn new(
        args: &Args,
        sample_rate: u32,
        hop: usize,
    ) -> Result<Self> {

        let pitch = Pitch::new(
            PitchMode::Yin,
            args.pitch_buffer,
            hop,
            sample_rate,
        )?
        .with_unit(PitchUnit::Midi)
        .with_silence(args.silence_db);


        // HFC works well for sharp keyboard attacks.
        let onset = Onset::new(
            OnsetMode::Hfc,
            args.onset_buffer,
            hop,
            sample_rate,
        )?
        .with_silence(args.silence_db)
        .with_threshold(args.onset_threshold)
        .with_minioi_ms(args.retrigger_ms);


        Ok(Self {
            pitch,
            onset,

            current_note: None,

            pending: None,

            fallback_note: None,
            fallback_count: 0,

            silence_count: 0,

            attack_ignore_hops:
                ms_to_hops(
                    args.attack_ignore_ms,
                    sample_rate,
                    hop,
                ),

            decision_hops:
                ms_to_hops(
                    args.decision_window_ms,
                    sample_rate,
                    hop,
                ),

            decision_extension_hops:
                ms_to_hops(
                    args.decision_extend_ms,
                    sample_rate,
                    hop,
                ),

            release_hops:
                ms_to_hops(
                    args.release_ms,
                    sample_rate,
                    hop,
                ),

            initial_stable:
                args.initial_stable,

            vote_ratio:
                args.vote_ratio,

            retrigger_time:
                Duration::from_secs_f32(
                    args.retrigger_ms / 1000.0
                ),

            last_trigger: None,

            silence_db:
                args.silence_db,

            debug:
                args.debug,

            started_at:
                Instant::now(),
        })
    }


    fn debug(
        &self,
        message: impl AsRef<str>,
    ) {
        if !self.debug {
            return;
        }

        let ms =
            self.started_at
                .elapsed()
                .as_secs_f64()
                * 1000.0;

        println!(
            "[{:10.2} ms] {}",
            ms,
            message.as_ref()
        );
    }


    fn send_new_note(
        &mut self,
        note: u8,
        onset_time: Option<Instant>,
        midi: &mut Midi,
    ) -> Result<()> {

        let now = Instant::now();

        match self.current_note {
            None => {
                midi.note_on(note)?;

                self.current_note =
                    Some(note);

                self.last_trigger =
                    Some(now);
            }

            Some(current)
                if current == note =>
            {
                // Same physical key pressed again.

                let Some(onset_time) =
                    onset_time
                else {
                    return Ok(());
                };

                let cooldown_ok =
                    self.last_trigger
                        .map(
                            |last| {
                                now.duration_since(last)
                                    >= self.retrigger_time
                            }
                        )
                        .unwrap_or(true);

                if !cooldown_ok {
                    return Ok(());
                }

                midi.note_off(note)?;
                midi.note_on(note)?;

                self.last_trigger =
                    Some(now);

                let latency =
                    now.duration_since(onset_time)
                        .as_secs_f64()
                        * 1000.0;

                self.debug(
                    format!(
                        "same-note retrigger {} ; onset->MIDI {:.1} ms",
                        note_name(note),
                        latency
                    )
                );
            }

            Some(current) => {
                midi.note_off(current)?;
                midi.note_on(note)?;

                self.current_note =
                    Some(note);

                self.last_trigger =
                    Some(now);

                if let Some(onset_time) =
                    onset_time
                {
                    let latency =
                        now.duration_since(onset_time)
                            .as_secs_f64()
                            * 1000.0;

                    self.debug(
                        format!(
                            "change {} -> {} ; onset->MIDI {:.1} ms",
                            note_name(current),
                            note_name(note),
                            latency
                        )
                    );
                }
            }
        }

        Ok(())
    }


    fn release(
        &mut self,
        midi: &mut Midi,
    ) -> Result<()> {

        if let Some(note) =
            self.current_note.take()
        {
            midi.note_off(note)?;
        }

        self.pending = None;

        self.fallback_note = None;
        self.fallback_count = 0;

        Ok(())
    }


    fn process(
        &mut self,
        frame: &[f32],
        midi: &mut Midi,
    ) -> Result<()> {

        let now =
            Instant::now();

        let level =
            rms_db(frame);

        let midi_float =
            self.pitch
                .do_result(frame)?;

        let detected =
            quantize_pitch(midi_float);

        let onset_value =
            self.onset
                .do_result(frame)?;

        let onset_detected =
            onset_value > 0.0;


        // -------------------------------------------------
        // Silence / release
        // -------------------------------------------------

        if level < self.silence_db {
            self.silence_count += 1;

            self.fallback_note = None;
            self.fallback_count = 0;

            if self.silence_count
                >= self.release_hops
            {
                self.release(midi)?;
            }

            if self.debug {
                self.debug(
                    format!(
                        "level={:6.1} dB silence",
                        level
                    )
                );
            }

            return Ok(());
        }

        self.silence_count = 0;


        // -------------------------------------------------
        // New attack
        // -------------------------------------------------

        if onset_detected {
            self.pending =
                Some(
                    PendingDecision::new(
                        now,
                        self.attack_ignore_hops,
                        self.decision_hops,
                        self.decision_extension_hops,
                    )
                );

            self.fallback_note = None;
            self.fallback_count = 0;

            self.debug(
                format!(
                    "ONSET level={:.1} dB pitch={:.2}",
                    level,
                    midi_float
                )
            );
        }


        // -------------------------------------------------
        // Attack decision state
        // -------------------------------------------------

        if let Some(mut pending) =
            self.pending.take()
        {
            if pending.ignore_left > 0 {
                pending.ignore_left -= 1;

                self.pending = Some(pending);

                if self.debug {
                    self.debug(
                        format!(
                            "attack-ignore pitch={:.2}",
                            midi_float
                        )
                    );
                }

                return Ok(());
            }


            if let Some((note, cents)) =
                detected
            {
                pending.vote(note);

                if self.debug {
                    self.debug(
                        format!(
                            "vote {} ({:+.1}c)",
                            note_name(note),
                            cents
                        )
                    );
                }
            }


            if pending.collect_left > 0 {
                pending.collect_left -= 1;
            }


            if pending.collect_left > 0 {
                self.pending = Some(pending);

                return Ok(());
            }


            let decision =
                pending.winner();

            let onset_at =
                pending.onset_at;

            let extensions_left =
                pending.extensions_left;


            if let Some(
                (
                    winner,
                    count,
                    ratio
                )
            ) = decision
            {
                self.debug(
                    format!(
                        "decision {} votes={} ratio={:.2}",
                        note_name(winner),
                        count,
                        ratio
                    )
                );

                // Need at least two real pitch
                // observations and a dominant winner.
                if count >= 2
                    && ratio >= self.vote_ratio
                {
                    self.send_new_note(
                        winner,
                        Some(onset_at),
                        midi,
                    )?;

                    self.fallback_note = None;
                    self.fallback_count = 0;

                    return Ok(());
                }
            }


            // One short extension for ambiguous
            // attacks instead of immediately accepting
            // a harmonic.
            if extensions_left > 0 {
                pending.extensions_left -= 1;

                pending.collect_left =
                    pending.extension_hops;

                self.pending = Some(pending);

                self.debug(
                    "ambiguous onset; extending decision window"
                );

                return Ok(());
            }


            self.debug(
                "onset decision rejected"
            );

            self.pending = None;

            return Ok(());
        }


        // -------------------------------------------------
        // Fallback
        //
        // Only acquire the first note when no MIDI note is
        // active. Once active, note changes require an onset.
        // -------------------------------------------------

        let Some((note, cents)) =
            detected
        else {
            self.fallback_note = None;
            self.fallback_count = 0;

            return Ok(());
        };


        if self.current_note.is_some() {
            self.fallback_note = None;
            self.fallback_count = 0;

            if self.debug && self.current_note != Some(note) {
                self.debug(
                    format!(
                        "ignoring pitch change without onset: {} -> {} ({:+.1}c)",
                        note_name(self.current_note.unwrap()),
                        note_name(note),
                        cents,
                    )
                );
            }

            return Ok(());
        }


        // A released key can decay near the silence threshold
        // long enough to look stable. Fallback only starts a note
        // when the level is clearly above that tail.
        if level < self.silence_db + FALLBACK_ATTACK_MARGIN_DB {
            self.fallback_note = None;
            self.fallback_count = 0;

            return Ok(());
        }


        if self.fallback_note == Some(note) {
            self.fallback_count += 1;
        } else {
            self.fallback_note =
                Some(note);

            self.fallback_count =
                1;
        }


        if self.debug {
            self.debug(
                format!(
                    "initial fallback {} {}/{} ({:+.1}c)",
                    note_name(note),
                    self.fallback_count,
                    self.initial_stable,
                    cents
                )
            );
        }


        if self.fallback_count >= self.initial_stable {
            self.send_new_note(
                note,
                None,
                midi,
            )?;

            self.fallback_note = None;
            self.fallback_count = 0;
        }


        Ok(())
    }


    fn shutdown(
        &mut self,
        midi: &mut Midi,
    ) -> Result<()> {
        self.release(midi)
    }
}


// ---------------------------------------------------------
// ALSA capture
// ---------------------------------------------------------

fn open_capture(
    args: &Args,
) -> Result<(PCM, u32, usize)> {

    let pcm =
        PCM::new(
            &args.device,
            Direction::Capture,
            false,
        )
        .with_context(
            || format!(
                "Cannot open ALSA capture PCM '{}'",
                args.device
            )
        )?;


    let (
        actual_rate,
        actual_period,
    );

    {
        let hwp =
            HwParams::any(&pcm)?;

        hwp.set_access(
            Access::RWInterleaved
        )?;

        hwp.set_format(
            Format::s16()
        )?;

        hwp.set_channels(1)?;

        actual_rate =
            hwp.set_rate_near(
                args.sample_rate,
                ValueOr::Nearest,
            )?;

        actual_period =
            hwp.set_period_size_near(
                args.hop as i64,
                ValueOr::Nearest,
            )?;


        // A few periods of capture buffering.
        // ALSA/PipeWire may adjust this.
        let _ =
            hwp.set_buffer_size_near(
                actual_period * 4
            );


        pcm.hw_params(&hwp)?;
    }


    pcm.prepare()?;


    Ok((
        pcm,
        actual_rate,
        actual_period as usize,
    ))
}


// ---------------------------------------------------------
// Main
// ---------------------------------------------------------

fn main() -> Result<()> {
    let args =
        Args::parse();


    let running =
        Arc::new(
            AtomicBool::new(true)
        );

    {
        let running =
            Arc::clone(&running);

        ctrlc::set_handler(
            move || {
                running.store(
                    false,
                    Ordering::SeqCst
                );
            }
        )?;
    }


    let (
        pcm,
        sample_rate,
        hop,
    ) =
        open_capture(&args)?;


    let (
        buffer_frames,
        period_frames,
    ) =
        pcm.get_params()?;


    println!(
        "PSS-F30 -> Rust/aubio -> MIDI"
    );

    println!(
        "--------------------------------"
    );

    println!(
        "ALSA input       : {}",
        args.device
    );

    println!(
        "Sample rate      : {}",
        sample_rate
    );

    println!(
        "Hop / period     : {} samples ({:.2} ms)",
        hop,
        hop as f64
            / sample_rate as f64
            * 1000.0
    );

    println!(
        "ALSA buffer      : {} frames",
        buffer_frames
    );

    println!(
        "ALSA period      : {} frames",
        period_frames
    );

    println!(
        "Pitch buffer     : {} samples ({:.2} ms)",
        args.pitch_buffer,
        args.pitch_buffer as f64
            / sample_rate as f64
            * 1000.0
    );

    println!(
        "Range            : C2-C5 / MIDI 36-72"
    );

    println!(
        "MIDI output      : PSS-F30 Audio MIDI"
    );

    println!();


    if sample_rate != args.sample_rate {
        eprintln!(
            "Warning: requested {} Hz, ALSA selected {} Hz",
            args.sample_rate,
            sample_rate
        );
    }


    if hop != args.hop {
        eprintln!(
            "Warning: requested hop {}, ALSA selected {}",
            args.hop,
            hop
        );
    }


    let mut detector =
        Detector::new(
            &args,
            sample_rate,
            hop,
        )?;


    let mut midi =
        Midi::new()?;


    // Capture as signed 16-bit.
    let io =
        pcm.io_i16()?;


    let mut raw =
        vec![0i16; hop];

    let mut frame =
        vec![0.0f32; hop];

    let mut frame_fill =
        0usize;


    println!(
        "Running. Ctrl+C to stop."
    );

    println!();


    while running.load(
        Ordering::SeqCst
    ) {
        match io.readi(
            &mut raw
        ) {
            Ok(frames_read) => {

                for &sample
                    in &raw[..frames_read]
                {
                    frame[frame_fill] =
                        sample as f32
                        / 32768.0;

                    frame_fill += 1;


                    if frame_fill == hop {
                        detector.process(
                            &frame,
                            &mut midi,
                        )?;

                        frame_fill = 0;
                    }
                }
            }


            Err(err) => {
                eprintln!(
                    "ALSA capture error: {err}; trying recovery"
                );

                pcm.try_recover(
                    err,
                    false
                )?;
            }
        }
    }


    detector.shutdown(
        &mut midi
    )?;


    println!(
        "Stopped."
    );


    Ok(())
}


// ---------------------------------------------------------
// Tests for vote logic
// ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;


    fn decision(
        notes: &[u8]
    ) -> Option<u8> {
        let mut p =
            PendingDecision::new(
                Instant::now(),
                0,
                notes.len(),
                0,
            );

        for &note in notes {
            p.vote(note);
        }

        p.winner()
            .map(
                |(note, _, _)|
                note
            )
    }


    #[test]
    fn stable_c3() {
        assert_eq!(
            decision(
                &[48, 48, 48, 48]
            ),
            Some(48)
        );
    }


    #[test]
    fn attack_harmonics_choose_c3() {
        assert_eq!(
            decision(
                &[
                    64,
                    40,
                    48,
                    48,
                    48,
                    48,
                ]
            ),
            Some(48)
        );
    }


    #[test]
    fn transient_does_not_dominate() {
        assert_eq!(
            decision(
                &[
                    48,
                    48,
                    40,
                    72,
                    48,
                    48,
                ]
            ),
            Some(48)
        );
    }
}
