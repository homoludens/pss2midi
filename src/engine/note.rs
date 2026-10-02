//! Supported MIDI note mapping and audio-level helpers.

pub const MIN_MIDI: i32 = 36; // C2
pub const MAX_MIDI: i32 = 72; // C5
pub const NOTE_COUNT: usize = (MAX_MIDI - MIN_MIDI + 1) as usize;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn note_name(note: u8) -> String {
    let octave = note as i32 / 12 - 1;
    format!("{}{}", NOTE_NAMES[note as usize % 12], octave)
}

pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }

    let power = samples.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / samples.len() as f64;
    let rms = power.sqrt();

    if rms <= 1e-12 {
        -120.0
    } else {
        (20.0 * rms.log10()) as f32
    }
}

pub fn ms_to_hops(ms: f32, rate: u32, hop: usize) -> usize {
    let samples = ms * rate as f32 / 1000.0;
    ((samples / hop as f32).ceil() as usize).max(1)
}

pub fn quantize_pitch(midi: f32) -> Option<(u8, f32)> {
    if !midi.is_finite() || midi <= 0.0 {
        return None;
    }

    let rounded = midi.round() as i32;
    if !(MIN_MIDI..=MAX_MIDI).contains(&rounded) {
        return None;
    }

    let cents = (midi - rounded as f32) * 100.0;
    (cents.abs() <= 45.0).then_some((rounded as u8, cents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_follow_midi_pitch_class_and_octave() {
        let notes = [
            (36, "C2"),
            (37, "C#2"),
            (38, "D2"),
            (39, "D#2"),
            (40, "E2"),
            (41, "F2"),
            (42, "F#2"),
            (43, "G2"),
            (44, "G#2"),
            (45, "A2"),
            (46, "A#2"),
            (47, "B2"),
            (48, "C3"),
            (49, "C#3"),
            (60, "C4"),
            (72, "C5"),
        ];

        for (midi_note, expected) in notes {
            assert_eq!(note_name(midi_note), expected, "MIDI note {midi_note}");
        }
    }

    #[test]
    fn quantizer_keeps_pss_f30_range_c2_through_c5() {
        assert_eq!(quantize_pitch(36.0).map(|(note, _)| note), Some(36));
        assert_eq!(quantize_pitch(72.0).map(|(note, _)| note), Some(72));
        assert_eq!(quantize_pitch(35.0), None);
        assert_eq!(quantize_pitch(73.0), None);
    }
}
