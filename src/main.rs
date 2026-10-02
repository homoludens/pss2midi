use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::Result;
use clap::Parser;

use pss2midi::engine::{
    audio::open_capture,
    calibration,
    config::{Cli, Commands, DetectorMode, RunArgs},
    detector::Detector,
    midi::Midi,
};

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Commands::Calibrate(args)) = cli.command {
        return calibration::run(args);
    }
    run(cli.run)
}

fn run(args: RunArgs) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let signal_running = Arc::clone(&running);
    ctrlc::set_handler(move || signal_running.store(false, Ordering::SeqCst))?;

    let (pcm, sample_rate, hop) = open_capture(&args.audio)?;
    let (buffer_frames, period_frames) = pcm.get_params()?;
    print_startup(&args, sample_rate, hop, buffer_frames, period_frames);

    let mut detector = Detector::new(&args, sample_rate, hop)?;
    let mut midi = Midi::new()?;
    let io = pcm.io_i16()?;
    let mut raw = vec![0i16; hop];
    let mut frame = vec![0.0f32; hop];
    let mut frame_fill = 0;

    println!("Running. Ctrl+C to stop.\n");
    while running.load(Ordering::SeqCst) {
        match io.readi(&mut raw) {
            Ok(frames_read) => {
                for &sample in &raw[..frames_read] {
                    frame[frame_fill] = sample as f32 / 32768.0;
                    frame_fill += 1;
                    if frame_fill == hop {
                        detector.process(&frame, &mut midi)?;
                        frame_fill = 0;
                    }
                }
            }
            Err(error) => {
                eprintln!("ALSA capture error: {error}; trying recovery");
                pcm.try_recover(error, false)?;
            }
        }
    }

    detector.shutdown(&mut midi)?;
    println!("Stopped.");
    Ok(())
}

fn print_startup(
    args: &RunArgs,
    sample_rate: u32,
    hop: usize,
    buffer_frames: u64,
    period_frames: u64,
) {
    println!("PSS-F30 -> Rust/aubio -> MIDI");
    println!("--------------------------------");
    println!("Detector         : {:?}", args.detector);
    println!("ALSA input       : {}", args.audio.device);
    println!("Sample rate      : {sample_rate}");
    println!(
        "Hop / period     : {hop} samples ({:.2} ms)",
        hop as f64 / sample_rate as f64 * 1000.0
    );
    println!("ALSA buffer      : {buffer_frames} frames");
    println!("ALSA period      : {period_frames} frames");
    if args.detector != DetectorMode::Spectral {
        println!(
            "YIN pitch buffer : {} samples ({:.2} ms)",
            args.pitch_buffer,
            args.pitch_buffer as f64 / sample_rate as f64 * 1000.0
        );
    }
    println!("Range            : C2-C5 / MIDI 36-72");
    println!("MIDI output      : PSS-F30 Audio MIDI\n");

    if args.detector != DetectorMode::Yin {
        println!(
            "Spectral window : {:.1} ms delay + {:.1} ms samples (FFT {})",
            args.spectral.spectral_delay_ms,
            args.spectral.spectral_window_ms,
            args.spectral.fft_size,
        );
    }

    if sample_rate != args.audio.sample_rate {
        eprintln!(
            "Warning: requested {} Hz, ALSA selected {sample_rate} Hz",
            args.audio.sample_rate
        );
    }
    if hop != args.audio.hop {
        eprintln!(
            "Warning: requested hop {}, ALSA selected {hop}",
            args.audio.hop
        );
    }
}
