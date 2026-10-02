use std::io::{self, Write};

use anyhow::{ensure, Context, Result};

use crate::{
    audio::{open_capture, AudioFrameReader},
    config::{default_template_path, CalibrateArgs},
    features::{FeatureExtractor, SampleCapture},
    note::{note_name, rms_db, MAX_MIDI, MIN_MIDI},
    onset::OnsetDetector,
    templates::TemplateFile,
};

pub fn run(args: CalibrateArgs) -> Result<()> {
    ensure!(
        args.samples_per_note > 0,
        "--samples-per-note must be greater than zero"
    );
    ensure!(
        args.spectral.spectral_delay_ms.is_finite() && args.spectral.spectral_delay_ms >= 0.0,
        "--spectral-delay-ms must be a finite non-negative number"
    );
    let output = args.output.unwrap_or_else(default_template_path);
    let (pcm, sample_rate, hop) = open_capture(&args.audio)?;
    let mut reader = AudioFrameReader::new(&pcm, hop)?;
    let mut onset = OnsetDetector::new(
        &args.audio,
        args.audio.onset_buffer,
        0.30,
        60.0,
        sample_rate,
        hop,
    )?;
    let mut extractor = FeatureExtractor::new(
        sample_rate,
        args.spectral.spectral_window_ms,
        args.spectral.fft_size,
    )?;
    let delay_samples =
        (sample_rate as f32 * args.spectral.spectral_delay_ms / 1000.0).round() as u64;
    let mut templates = TemplateFile::empty(
        sample_rate,
        args.spectral.fft_size,
        args.spectral.spectral_window_ms,
        args.spectral.spectral_delay_ms,
    );

    println!("PSS-F30 spectral calibration");
    println!("Audio input: {}", args.audio.device);
    println!("Output: {}", output.display());
    println!("Samples per note: {}\n", args.samples_per_note);

    let stdin = io::stdin();
    for midi_note in MIN_MIDI..=MAX_MIDI {
        let note = midi_note as u8;
        let label = note_name(note);
        let mut examples = Vec::with_capacity(args.samples_per_note);
        while examples.len() < args.samples_per_note {
            print!(
                "Press Enter to arm {label} (MIDI {note}) sample {}/{}; then play that key: ",
                examples.len() + 1,
                args.samples_per_note
            );
            io::stdout().flush()?;
            let mut line = String::new();
            stdin
                .read_line(&mut line)
                .context("Could not read calibration prompt")?;

            let waveform = capture_one(
                &mut reader,
                &mut onset,
                hop,
                delay_samples,
                extractor.window_samples(),
            )?;
            let level = rms_db(&waveform);
            let peak = waveform
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            if level < args.audio.silence_db {
                println!("  rejected: captured level {level:.1} dBFS is below silence threshold");
                continue;
            }
            if peak >= 0.98 {
                println!("  warning: peak {peak:.3}; possible clipping");
            }
            let feature = extractor.extract(&waveform)?;
            examples.push(feature);
            println!(
                "  {label} sample {}/{} accepted; RMS {level:.1} dBFS, peak {peak:.3}",
                examples.len(),
                args.samples_per_note
            );
        }
        templates.notes.insert(note, examples);
        println!("{label} saved.\n");
    }

    templates.save(&output)?;
    let count = templates.notes.values().map(Vec::len).sum::<usize>();
    println!("Calibration complete.");
    println!("Saved {count} samples to {}", output.display());
    Ok(())
}

fn capture_one(
    reader: &mut AudioFrameReader<'_>,
    onset: &mut OnsetDetector,
    hop: usize,
    delay_samples: u64,
    window_samples: usize,
) -> Result<Vec<f32>> {
    let mut frame = vec![0.0f32; hop];
    let mut capture: Option<SampleCapture> = None;

    loop {
        let frame_start = reader.next_frame(&mut frame)?;
        if capture.is_none() && onset.detect(&frame)? {
            println!("  onset detected; collecting spectral window");
            capture = Some(SampleCapture::new(
                frame_start + delay_samples,
                window_samples,
            ));
        }
        if let Some(active_capture) = capture.as_mut() {
            active_capture.push_frame(frame_start, &frame);
            if active_capture.is_complete() {
                return Ok(active_capture.samples().to_vec());
            }
        }
    }
}
