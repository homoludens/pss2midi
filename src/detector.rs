use std::time::{Duration, Instant};

use anyhow::Result;
use aubio::{Onset, OnsetMode, Pitch, PitchMode, PitchUnit};

use crate::{
    config::Args,
    midi::Midi,
    note::{ms_to_hops, note_name, quantize_pitch, rms_db, MIN_MIDI, NOTE_COUNT},
};

const FALLBACK_ATTACK_MARGIN_DB: f32 = 10.0;

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
        if index >= 0 && (index as usize) < NOTE_COUNT {
            self.votes[index as usize] += 1;
            self.total_votes += 1;
        }
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
        let ratio = *count as f32 / self.total_votes as f32;
        Some(((MIN_MIDI + index as i32) as u8, *count, ratio))
    }
}

pub struct Detector {
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
    pub fn new(args: &Args, sample_rate: u32, hop: usize) -> Result<Self> {
        let pitch = Pitch::new(PitchMode::Yin, args.pitch_buffer, hop, sample_rate)?
            .with_unit(PitchUnit::Midi)
            .with_silence(args.silence_db);
        let onset = Onset::new(OnsetMode::Hfc, args.onset_buffer, hop, sample_rate)?
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
            attack_ignore_hops: ms_to_hops(args.attack_ignore_ms, sample_rate, hop),
            decision_hops: ms_to_hops(args.decision_window_ms, sample_rate, hop),
            decision_extension_hops: ms_to_hops(args.decision_extend_ms, sample_rate, hop),
            release_hops: ms_to_hops(args.release_ms, sample_rate, hop),
            initial_stable: args.initial_stable,
            vote_ratio: args.vote_ratio,
            retrigger_time: Duration::from_secs_f32(args.retrigger_ms / 1000.0),
            last_trigger: None,
            silence_db: args.silence_db,
            debug: args.debug,
            started_at: Instant::now(),
        })
    }

    fn debug(&self, message: impl AsRef<str>) {
        if self.debug {
            println!(
                "[{:10.2} ms] {}",
                self.started_at.elapsed().as_secs_f64() * 1000.0,
                message.as_ref()
            );
        }
    }

    fn clear_fallback(&mut self) {
        self.fallback_note = None;
        self.fallback_count = 0;
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
                self.current_note = Some(note);
                self.last_trigger = Some(now);
            }
            Some(current) if current == note => {
                let Some(onset_time) = onset_time else {
                    return Ok(());
                };
                let cooldown_ok = self
                    .last_trigger
                    .map(|last| now.duration_since(last) >= self.retrigger_time)
                    .unwrap_or(true);
                if !cooldown_ok {
                    return Ok(());
                }

                midi.note_off(note)?;
                midi.note_on(note)?;
                self.last_trigger = Some(now);
                self.debug(format!(
                    "same-note retrigger {} ; onset->MIDI {:.1} ms",
                    note_name(note),
                    now.duration_since(onset_time).as_secs_f64() * 1000.0
                ));
            }
            Some(current) => {
                midi.note_off(current)?;
                midi.note_on(note)?;
                self.current_note = Some(note);
                self.last_trigger = Some(now);

                if let Some(onset_time) = onset_time {
                    self.debug(format!(
                        "change {} -> {} ; onset->MIDI {:.1} ms",
                        note_name(current),
                        note_name(note),
                        now.duration_since(onset_time).as_secs_f64() * 1000.0
                    ));
                }
            }
        }

        Ok(())
    }

    fn release(&mut self, midi: &mut Midi) -> Result<()> {
        if let Some(note) = self.current_note.take() {
            midi.note_off(note)?;
        }
        self.pending = None;
        self.clear_fallback();
        Ok(())
    }

    pub fn process(&mut self, frame: &[f32], midi: &mut Midi) -> Result<()> {
        let now = Instant::now();
        let level = rms_db(frame);
        let midi_float = self.pitch.do_result(frame)?;
        let detected = quantize_pitch(midi_float);
        let onset_detected = self.onset.do_result(frame)? > 0.0;

        if level < self.silence_db {
            self.silence_count += 1;
            self.clear_fallback();
            if self.silence_count >= self.release_hops {
                self.release(midi)?;
            }
            self.debug(format!("level={level:6.1} dB silence"));
            return Ok(());
        }
        self.silence_count = 0;

        if onset_detected {
            self.pending = Some(PendingDecision::new(
                now,
                self.attack_ignore_hops,
                self.decision_hops,
                self.decision_extension_hops,
            ));
            self.clear_fallback();
            self.debug(format!("ONSET level={level:.1} dB pitch={midi_float:.2}"));
        }

        if let Some(mut pending) = self.pending.take() {
            if pending.ignore_left > 0 {
                pending.ignore_left -= 1;
                self.pending = Some(pending);
                self.debug(format!("attack-ignore pitch={midi_float:.2}"));
                return Ok(());
            }

            if let Some((note, cents)) = detected {
                pending.vote(note);
                self.debug(format!("vote {} ({cents:+.1}c)", note_name(note)));
            }
            pending.collect_left = pending.collect_left.saturating_sub(1);
            if pending.collect_left > 0 {
                self.pending = Some(pending);
                return Ok(());
            }

            let decision = pending.winner();
            let onset_at = pending.onset_at;
            if let Some((winner, count, ratio)) = decision {
                self.debug(format!(
                    "decision {} votes={count} ratio={ratio:.2}",
                    note_name(winner)
                ));
                if count >= 2 && ratio >= self.vote_ratio {
                    self.send_new_note(winner, Some(onset_at), midi)?;
                    self.clear_fallback();
                    return Ok(());
                }
            }

            if pending.extensions_left > 0 {
                pending.extensions_left -= 1;
                pending.collect_left = pending.extension_hops;
                self.pending = Some(pending);
                self.debug("ambiguous onset; extending decision window");
                return Ok(());
            }

            self.debug("onset decision rejected");
            return Ok(());
        }

        let Some((note, cents)) = detected else {
            self.clear_fallback();
            return Ok(());
        };

        if let Some(current) = self.current_note {
            self.clear_fallback();
            if current != note {
                self.debug(format!(
                    "ignoring pitch change without onset: {} -> {} ({cents:+.1}c)",
                    note_name(current),
                    note_name(note)
                ));
            }
            return Ok(());
        }

        if level < self.silence_db + FALLBACK_ATTACK_MARGIN_DB {
            self.clear_fallback();
            return Ok(());
        }

        if self.fallback_note == Some(note) {
            self.fallback_count += 1;
        } else {
            self.fallback_note = Some(note);
            self.fallback_count = 1;
        }
        self.debug(format!(
            "initial fallback {} {}/{} ({cents:+.1}c)",
            note_name(note),
            self.fallback_count,
            self.initial_stable
        ));

        if self.fallback_count >= self.initial_stable {
            self.send_new_note(note, None, midi)?;
            self.clear_fallback();
        }
        Ok(())
    }

    pub fn shutdown(&mut self, midi: &mut Midi) -> Result<()> {
        self.release(midi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(notes: &[u8]) -> Option<u8> {
        let mut pending = PendingDecision::new(Instant::now(), 0, notes.len(), 0);
        for &note in notes {
            pending.vote(note);
        }
        pending.winner().map(|(note, _, _)| note)
    }

    #[test]
    fn stable_c3() {
        assert_eq!(decision(&[48, 48, 48, 48]), Some(48));
    }

    #[test]
    fn attack_harmonics_choose_c3() {
        assert_eq!(decision(&[64, 40, 48, 48, 48, 48]), Some(48));
    }

    #[test]
    fn transient_does_not_dominate() {
        assert_eq!(decision(&[48, 48, 40, 72, 48, 48]), Some(48));
    }
}
