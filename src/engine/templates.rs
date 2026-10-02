use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::{
    features::FeatureExtractor,
    note::{MAX_MIDI, MIN_MIDI},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateFile {
    pub version: u32,
    pub device: String,
    pub sample_rate: u32,
    pub fft_size: usize,
    pub window_ms: f32,
    pub delay_ms: f32,
    pub notes: BTreeMap<u8, Vec<Vec<f32>>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub note: u8,
    pub score: f32,
    pub second_score: f32,
    pub margin: f32,
    pub accepted: bool,
}

impl TemplateFile {
    pub fn empty(sample_rate: u32, fft_size: usize, window_ms: f32, delay_ms: f32) -> Self {
        Self {
            version: 1,
            device: "Yamaha PSS-F30".to_owned(),
            sample_rate,
            fft_size,
            window_ms,
            delay_ms,
            notes: BTreeMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Cannot read spectral templates from {}", path.display()))?;
        serde_json::from_str(&contents)
            .with_context(|| format!("Invalid spectral template JSON: {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Cannot create template directory {}", parent.display())
                })?;
            }
        }
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(path, format!("{contents}\n"))
            .with_context(|| format!("Cannot save templates to {}", path.display()))
    }

    pub fn validate(
        &self,
        sample_rate: u32,
        fft_size: usize,
        window_ms: f32,
        delay_ms: f32,
        feature_len: usize,
    ) -> Result<()> {
        ensure!(
            self.version == 1,
            "unsupported template file version {}",
            self.version
        );
        ensure!(
            self.sample_rate == sample_rate,
            "template sample rate {} does not match input {sample_rate}",
            self.sample_rate
        );
        ensure!(
            self.fft_size == fft_size,
            "template FFT size {} does not match --fft-size {fft_size}",
            self.fft_size
        );
        ensure!(
            (self.window_ms - window_ms).abs() < 0.001,
            "template window does not match --spectral-window-ms"
        );
        ensure!(
            (self.delay_ms - delay_ms).abs() < 0.001,
            "template delay does not match --spectral-delay-ms"
        );

        for note in MIN_MIDI..=MAX_MIDI {
            let examples = self
                .notes
                .get(&(note as u8))
                .ok_or_else(|| anyhow::anyhow!("template file is missing MIDI note {note}"))?;
            ensure!(
                !examples.is_empty(),
                "template MIDI note {note} has no examples"
            );
            for (example_index, example) in examples.iter().enumerate() {
                ensure!(
                    example.len() == feature_len,
                    "MIDI note {note} example {} has {} features, expected {feature_len}",
                    example_index + 1,
                    example.len()
                );
                ensure!(
                    example.iter().all(|value| value.is_finite()),
                    "MIDI note {note} contains a non-finite feature"
                );
            }
        }
        Ok(())
    }

    pub fn classify(
        &self,
        observation: &[f32],
        minimum_score: f32,
        minimum_margin: f32,
    ) -> Result<Classification> {
        let mut scores = Vec::with_capacity((MAX_MIDI - MIN_MIDI + 1) as usize);
        for note in MIN_MIDI..=MAX_MIDI {
            let examples = self
                .notes
                .get(&(note as u8))
                .ok_or_else(|| anyhow::anyhow!("template file is missing MIDI note {note}"))?;
            let mut best_example = f32::NEG_INFINITY;
            for template in examples {
                ensure!(
                    template.len() == observation.len(),
                    "template feature length mismatch for MIDI note {note}"
                );
                let similarity = cosine_similarity(observation, template)?;
                best_example = best_example.max(similarity);
            }
            scores.push((note as u8, best_example));
        }
        scores.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        if scores.len() < 2 {
            bail!("at least two note templates are required");
        }
        let (note, score) = scores[0];
        let second_score = scores[1].1;
        let margin = score - second_score;
        Ok(Classification {
            note,
            score,
            second_score,
            margin,
            accepted: score >= minimum_score && margin >= minimum_margin,
        })
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f32> {
    ensure!(
        left.len() == right.len(),
        "feature vectors have different lengths"
    );
    ensure!(!left.is_empty(), "feature vectors are empty");
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    ensure!(
        left_norm > 1e-12 && right_norm > 1e-12,
        "feature vector has zero norm"
    );
    Ok(dot / (left_norm * right_norm))
}

pub fn validate_for_extractor(
    templates: &TemplateFile,
    extractor: &FeatureExtractor,
    delay_ms: f32,
) -> Result<()> {
    templates.validate(
        extractor.sample_rate(),
        extractor.fft_size(),
        extractor.window_ms(),
        delay_ms,
        extractor.feature_len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_templates(feature_len: usize, note_vectors: &[(u8, Vec<f32>)]) -> TemplateFile {
        let mut templates = TemplateFile::empty(48_000, 2048, 30.0, 8.0);
        for note in MIN_MIDI..=MAX_MIDI {
            let vector = note_vectors
                .iter()
                .find(|(key, _)| *key == note as u8)
                .map(|(_, vector)| vector.clone())
                .unwrap_or_else(|| vec![0.0, 0.0, 1.0][..feature_len].to_vec());
            templates.notes.insert(note as u8, vec![vector]);
        }
        templates
    }

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let result = cosine_similarity(&[0.6, 0.8], &[0.6, 0.8]).unwrap();
        assert!((result - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_distinguishes_orthogonal_vectors() {
        let result = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(result < 0.01);
    }

    #[test]
    fn classifier_reports_winner_second_and_margin_then_rejects_ambiguous() {
        let templates = complete_templates(
            3,
            &[
                (48, vec![1.0, 0.0, 0.0]),
                (49, vec![0.8, 0.6, 0.0]),
                (50, vec![0.0, 1.0, 0.0]),
            ],
        );
        let clear = templates.classify(&[1.0, 0.0, 0.0], 0.75, 0.03).unwrap();
        assert_eq!(clear.note, 48);
        assert!((clear.second_score - 0.8).abs() < 1e-6);
        assert!((clear.margin - 0.2).abs() < 1e-6);
        assert!(clear.accepted);

        let ambiguous = templates
            .classify(&[0.9, 0.4358899, 0.0], 0.75, 0.2)
            .unwrap();
        assert!(!ambiguous.accepted);
    }

    #[test]
    fn template_json_round_trip() {
        let templates = complete_templates(3, &[]);
        let json = serde_json::to_string(&templates).unwrap();
        let decoded: TemplateFile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.notes.len(), 37);
    }

    #[test]
    fn template_file_saves_and_loads_all_note_examples() {
        let templates = complete_templates(3, &[]);
        let path = std::env::temp_dir().join(format!(
            "pss2midi-templates-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        templates.save(&path).unwrap();
        let loaded = TemplateFile::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(loaded.notes.len(), 37);
        assert_eq!(loaded.notes[&36].len(), 1);
    }

    #[test]
    fn validation_rejects_missing_midi_endpoint() {
        let mut templates = complete_templates(3, &[]);
        templates.notes.remove(&(MAX_MIDI as u8));
        assert!(templates.validate(48_000, 2048, 30.0, 8.0, 3).is_err());
    }

    #[test]
    fn calibration_and_runtime_extractor_metadata_must_match() {
        use crate::engine::features::FeatureExtractor;

        let extractor = FeatureExtractor::new(48_000, 30.0, 2048).unwrap();
        let mut templates = TemplateFile::empty(48_000, 2048, 30.0, 8.0);
        for note in MIN_MIDI..=MAX_MIDI {
            templates
                .notes
                .insert(note as u8, vec![vec![0.0; extractor.feature_len()]]);
        }
        assert!(validate_for_extractor(&templates, &extractor, 8.0).is_ok());
        assert!(validate_for_extractor(&templates, &extractor, 9.0).is_err());
    }

    #[test]
    fn harmonic_template_keeps_note_when_second_harmonic_is_strongest() {
        use crate::engine::features::FeatureExtractor;

        fn keyboard_tone(extractor: &FeatureExtractor, fundamental: f32, phase: f32) -> Vec<f32> {
            (0..extractor.window_samples())
                .map(|index| {
                    let time = index as f32 / extractor.sample_rate() as f32;
                    0.12 * (2.0 * std::f32::consts::PI * fundamental * time + phase).sin()
                        + 0.55
                            * (2.0 * std::f32::consts::PI * fundamental * 2.0 * time + phase).sin()
                        + 0.27
                            * (2.0 * std::f32::consts::PI * fundamental * 3.0 * time + phase).sin()
                        + 0.14
                            * (2.0 * std::f32::consts::PI * fundamental * 4.0 * time + phase).sin()
                })
                .collect()
        }

        let mut extractor = FeatureExtractor::new(48_000, 30.0, 2048).unwrap();
        let c3_wave = keyboard_tone(&extractor, 130.8128, 0.0);
        let c3_template = extractor.extract(&c3_wave).unwrap();
        let cs3_wave = keyboard_tone(&extractor, 138.5913, 0.2);
        let cs3_template = extractor.extract(&cs3_wave).unwrap();

        let mut templates = TemplateFile::empty(48_000, 2048, 30.0, 8.0);
        for note in MIN_MIDI..=MAX_MIDI {
            let mut quiet_reference = vec![0.0; extractor.feature_len()];
            quiet_reference[0] = 1.0;
            templates.notes.insert(note as u8, vec![quiet_reference]);
        }
        templates.notes.insert(48, vec![c3_template]);
        templates.notes.insert(49, vec![cs3_template]);

        let observation_wave = keyboard_tone(&extractor, 130.8128, 1.1);
        let observation = extractor.extract(&observation_wave).unwrap();
        let result = templates.classify(&observation, 0.75, 0.03).unwrap();
        assert_eq!(result.note, 48);
        assert!(
            result.accepted,
            "classification score and margin: {result:?}"
        );
    }
}
