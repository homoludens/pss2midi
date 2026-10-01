use anyhow::{Context, Result};
use midir::{os::unix::VirtualOutput, MidiOutput, MidiOutputConnection};

use crate::note::note_name;

pub struct Midi {
    conn: MidiOutputConnection,
}

impl Midi {
    pub fn new() -> Result<Self> {
        let output = MidiOutput::new("pss2midi").context("Cannot create MIDI client")?;
        let conn = output
            .create_virtual("PSS-F30 Audio MIDI")
            .map_err(|error| anyhow::anyhow!("Cannot create virtual MIDI output: {error}"))?;
        Ok(Self { conn })
    }

    pub fn note_on(&mut self, note: u8) -> Result<()> {
        self.conn
            .send(&[0x90, note, 100])
            .context("MIDI NOTE ON failed")?;
        println!("ON   {:4} MIDI={}", note_name(note), note);
        Ok(())
    }

    pub fn note_off(&mut self, note: u8) -> Result<()> {
        self.conn
            .send(&[0x80, note, 0])
            .context("MIDI NOTE OFF failed")?;
        println!("OFF  {:4} MIDI={}", note_name(note), note);
        Ok(())
    }
}
