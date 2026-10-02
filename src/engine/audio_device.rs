//! Engine-owned discovery and opening for audio capture devices.

use std::collections::BTreeMap;

use alsa::{device_name::HintIter, Direction};
use anyhow::Context;

use crate::engine::{
    audio::{open_capture_for_worker, AudioFrameSource},
    config::AudioArgs,
};

pub const PIPEWIRE_DEFAULT_DEVICE_ID: &str = "pipewire";

/// A capture device reported by ALSA. `id` is the ALSA PCM name used to open it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioInputDevice {
    pub id: String,
    pub label: String,
}

#[derive(Debug)]
pub(crate) struct AudioDeviceUnavailable {
    pub device_id: String,
    pub message: String,
}

pub(crate) struct OpenedAudioCapture {
    reader: Box<dyn AudioFrameSource>,
    pub sample_rate: u32,
    pub hop: usize,
}

impl OpenedAudioCapture {
    pub(crate) fn new<R: AudioFrameSource + 'static>(
        reader: R,
        sample_rate: u32,
        hop: usize,
    ) -> Self {
        Self {
            reader: Box::new(reader),
            sample_rate,
            hop,
        }
    }

    pub(crate) fn try_next_frame(
        &mut self,
        frame: &mut [f32],
        timeout: std::time::Duration,
    ) -> anyhow::Result<Option<u64>> {
        self.reader.try_next_frame(frame, timeout)
    }
}

/// Provider boundary for audio discovery and capture opening. The worker owns
/// the provider and all opened streams; UI callers only see device metadata.
pub(crate) trait AudioDeviceProvider: Send {
    fn enumerate_capture_devices(&mut self) -> Result<Vec<AudioInputDevice>, String>;

    fn open_capture(
        &mut self,
        device_id: &str,
        args: &AudioArgs,
    ) -> Result<OpenedAudioCapture, AudioDeviceUnavailable>;
}

#[derive(Default)]
pub(crate) struct AlsaAudioDeviceProvider;

impl AudioDeviceProvider for AlsaAudioDeviceProvider {
    fn enumerate_capture_devices(&mut self) -> Result<Vec<AudioInputDevice>, String> {
        let hints = HintIter::new_str(None, "pcm")
            .context("Cannot enumerate ALSA PCM devices")
            .map_err(|error| format!("{error:#}"))?;
        let mut devices = BTreeMap::new();

        for hint in hints {
            if hint.direction == Some(Direction::Playback) {
                continue;
            }
            let Some(id) = hint.name.filter(|id| !id.trim().is_empty()) else {
                continue;
            };
            let label = hint
                .desc
                .map(|description| {
                    description
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                        .join(" — ")
                })
                .filter(|description| !description.is_empty())
                .unwrap_or_else(|| id.clone());
            devices
                .entry(id.clone())
                .or_insert(AudioInputDevice { id, label });
        }

        Ok(devices.into_values().collect())
    }

    fn open_capture(
        &mut self,
        device_id: &str,
        args: &AudioArgs,
    ) -> Result<OpenedAudioCapture, AudioDeviceUnavailable> {
        let mut selected_args = args.clone();
        selected_args.device = device_id.to_owned();
        let (reader, sample_rate, hop) =
            open_capture_for_worker(&selected_args).map_err(|error| AudioDeviceUnavailable {
                device_id: device_id.to_owned(),
                message: format!("{error:#}"),
            })?;
        Ok(OpenedAudioCapture::new(reader, sample_rate, hop))
    }
}

pub(crate) fn choose_initial_device(
    devices: &[AudioInputDevice],
    saved_device_id: Option<&str>,
) -> String {
    if let Some(saved_device_id) =
        saved_device_id.filter(|saved_id| devices.iter().any(|device| device.id == *saved_id))
    {
        return saved_device_id.to_owned();
    }

    // Keep the established PipeWire ALSA PCM as the fallback even when ALSA's
    // hint list omits aliases. Do not fabricate a corresponding device entry.
    PIPEWIRE_DEFAULT_DEVICE_ID.to_owned()
}

pub(crate) fn is_selectable_device(devices: &[AudioInputDevice], device_id: &str) -> bool {
    device_id == PIPEWIRE_DEFAULT_DEVICE_ID || devices.iter().any(|device| device.id == device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, label: &str) -> AudioInputDevice {
        AudioInputDevice {
            id: id.to_owned(),
            label: label.to_owned(),
        }
    }

    #[test]
    fn saved_device_is_preferred_when_present_in_mock_enumeration() {
        let mock_enumeration = vec![
            device("hw:2,0", "USB Keyboard"),
            device(PIPEWIRE_DEFAULT_DEVICE_ID, "PipeWire default"),
        ];

        assert_eq!(
            choose_initial_device(&mock_enumeration, Some("hw:2,0")),
            "hw:2,0"
        );
    }

    #[test]
    fn pipewire_is_the_fallback_without_adding_a_synthetic_device() {
        let mock_enumeration = vec![device("hw:1,0", "Built-in capture")];

        assert_eq!(choose_initial_device(&mock_enumeration, None), "pipewire");
        assert_eq!(
            choose_initial_device(&mock_enumeration, Some("hw:missing,0")),
            "pipewire"
        );
        assert_eq!(mock_enumeration, vec![device("hw:1,0", "Built-in capture")]);
    }

    #[test]
    fn an_unavailable_selected_id_is_rejected_but_pipewire_can_be_retried() {
        let mock_enumeration = vec![device("hw:1,0", "Built-in capture")];

        assert!(!is_selectable_device(&mock_enumeration, "hw:gone,0"));
        assert!(is_selectable_device(&mock_enumeration, "pipewire"));

        let recovered_enumeration = vec![
            device("hw:1,0", "Built-in capture"),
            device("hw:gone,0", "Reconnected capture"),
        ];
        assert!(is_selectable_device(&recovered_enumeration, "hw:gone,0"));
    }
}
