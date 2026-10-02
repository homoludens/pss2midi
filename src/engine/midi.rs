use anyhow::{Context, Result};
use midir::{os::unix::VirtualOutput, MidiOutput, MidiOutputConnection};

use crate::engine::note::note_name;

pub const OUTPUT_NAME: &str = "PSS-F30 Audio MIDI";

pub struct Midi {
    conn: MidiOutputConnection,
}

/// Worker-owned MIDI operations, separated from device creation so lifecycle
/// tests can inject an in-memory output.
pub(crate) trait MidiPort {
    fn note_on(&mut self, note: u8) -> Result<()>;
    fn note_off(&mut self, note: u8) -> Result<()>;
}

pub(crate) trait MidiOutputProvider: Send {
    fn open_output(&mut self) -> Result<Box<dyn MidiPort>>;
}

#[derive(Default)]
pub(crate) struct SystemMidiOutputProvider;

impl MidiOutputProvider for SystemMidiOutputProvider {
    fn open_output(&mut self) -> Result<Box<dyn MidiPort>> {
        Ok(Box::new(Midi::new()?))
    }
}

impl Midi {
    pub fn new() -> Result<Self> {
        let output = MidiOutput::new("pss2midi").context("Cannot create MIDI client")?;
        let conn = output
            .create_virtual(OUTPUT_NAME)
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

impl MidiPort for Midi {
    fn note_on(&mut self, note: u8) -> Result<()> {
        Midi::note_on(self, note)
    }

    fn note_off(&mut self, note: u8) -> Result<()> {
        Midi::note_off(self, note)
    }
}
