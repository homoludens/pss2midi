//! Persistent desktop settings, independent of GPUI and audio processing.

use std::{
    error::Error,
    fmt, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::engine::config::{
    default_template_path, AudioArgs, DetectorMode, RunArgs, SpectralArgs,
};

static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// User-configurable audio input and requested capture rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioSettings {
    /// Preferred ALSA capture device; `pipewire` selects the PipeWire default.
    pub device: String,
    pub sample_rate: u32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            device: "pipewire".to_owned(),
            sample_rate: 48_000,
        }
    }
}

/// Common spectral detector parameters and classification thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpectralSettings {
    pub delay_ms: f32,
    pub window_ms: f32,
    pub minimum_score: f32,
    pub minimum_margin: f32,
}

impl Default for SpectralSettings {
    fn default() -> Self {
        Self {
            delay_ms: 8.0,
            window_ms: 30.0,
            minimum_score: 0.75,
            minimum_margin: 0.03,
        }
    }
}

/// Advanced DSP parameters corresponding to the current engine defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdvancedSettings {
    pub hop_size: usize,
    pub onset_buffer_size: usize,
    pub silence_db: f32,
    pub pitch_buffer_size: usize,
    pub attack_ignore_ms: f32,
    pub decision_window_ms: f32,
    pub decision_extend_ms: f32,
    pub release_ms: f32,
    pub retrigger_ms: f32,
    pub onset_threshold: f32,
    pub vote_ratio: f32,
    pub initial_stable_frames: usize,
    pub fft_size: usize,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        Self {
            hop_size: 128,
            onset_buffer_size: 1024,
            silence_db: -45.0,
            pitch_buffer_size: 2048,
            attack_ignore_ms: 10.0,
            decision_window_ms: 20.0,
            decision_extend_ms: 10.0,
            release_ms: 30.0,
            retrigger_ms: 60.0,
            onset_threshold: 0.30,
            vote_ratio: 0.60,
            initial_stable_frames: 10,
            fft_size: 2048,
        }
    }
}

/// Serde-backed settings for the desktop application.
///
/// Spectral samples are deliberately not stored here. `template_path` points
/// to the independent template file used by the existing classifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub audio: AudioSettings,
    pub detector_mode: DetectorMode,
    pub template_path: PathBuf,
    pub spectral: SpectralSettings,
    pub advanced: AdvancedSettings,
    /// Whether startup should attempt to start audio capture automatically.
    pub auto_start: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            audio: AudioSettings::default(),
            detector_mode: DetectorMode::Yin,
            template_path: default_template_path(),
            spectral: SpectralSettings::default(),
            advanced: AdvancedSettings::default(),
            auto_start: false,
        }
    }
}

impl AppConfig {
    /// Map persisted desktop preferences to the engine's runtime arguments.
    pub fn to_run_args(&self) -> RunArgs {
        RunArgs {
            audio: AudioArgs {
                device: self.audio.device.clone(),
                sample_rate: self.audio.sample_rate,
                hop: self.advanced.hop_size,
                onset_buffer: self.advanced.onset_buffer_size,
                silence_db: self.advanced.silence_db,
            },
            detector: self.detector_mode,
            pitch_buffer: self.advanced.pitch_buffer_size,
            attack_ignore_ms: self.advanced.attack_ignore_ms,
            decision_window_ms: self.advanced.decision_window_ms,
            decision_extend_ms: self.advanced.decision_extend_ms,
            release_ms: self.advanced.release_ms,
            retrigger_ms: self.advanced.retrigger_ms,
            onset_threshold: self.advanced.onset_threshold,
            vote_ratio: self.advanced.vote_ratio,
            initial_stable: self.advanced.initial_stable_frames,
            templates: Some(self.template_path.clone()),
            spectral: SpectralArgs {
                spectral_delay_ms: self.spectral.delay_ms,
                spectral_window_ms: self.spectral.window_ms,
                fft_size: self.advanced.fft_size,
                spectral_min_score: self.spectral.minimum_score,
                spectral_min_margin: self.spectral.minimum_margin,
            },
            debug: false,
        }
    }

    /// Validate settings that are active for the selected detector mode.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        let mut issues = Vec::new();

        if self.audio.device.trim().is_empty() {
            issues.push("audio.device must not be empty".to_owned());
        }
        if self.audio.sample_rate == 0 {
            issues.push("audio.sample_rate must be greater than zero".to_owned());
        }
        if self.template_path.as_os_str().is_empty() {
            issues.push("template_path must not be empty".to_owned());
        }

        let advanced = &self.advanced;
        if advanced.hop_size == 0 {
            issues.push("advanced.hop_size must be greater than zero".to_owned());
        }
        if advanced.onset_buffer_size == 0 {
            issues.push("advanced.onset_buffer_size must be greater than zero".to_owned());
        }
        if !advanced.silence_db.is_finite() {
            issues.push("advanced.silence_db must be finite".to_owned());
        }
        if !advanced.onset_threshold.is_finite() {
            issues.push("advanced.onset_threshold must be finite".to_owned());
        }
        require_nonnegative_finite("advanced.release_ms", advanced.release_ms, &mut issues);
        require_nonnegative_finite("advanced.retrigger_ms", advanced.retrigger_ms, &mut issues);

        let uses_yin = self.detector_mode != DetectorMode::Spectral;
        if uses_yin {
            if advanced.pitch_buffer_size == 0 {
                issues.push("advanced.pitch_buffer_size must be greater than zero".to_owned());
            }
            require_nonnegative_finite(
                "advanced.attack_ignore_ms",
                advanced.attack_ignore_ms,
                &mut issues,
            );
            require_nonnegative_finite(
                "advanced.decision_window_ms",
                advanced.decision_window_ms,
                &mut issues,
            );
            require_nonnegative_finite(
                "advanced.decision_extend_ms",
                advanced.decision_extend_ms,
                &mut issues,
            );
            if !advanced.vote_ratio.is_finite() || !(0.0..=1.0).contains(&advanced.vote_ratio) {
                issues.push("advanced.vote_ratio must be between 0 and 1".to_owned());
            }
            if advanced.initial_stable_frames == 0 {
                issues.push("advanced.initial_stable_frames must be greater than zero".to_owned());
            }
        }

        let uses_spectral = self.detector_mode != DetectorMode::Yin;
        if uses_spectral {
            require_nonnegative_finite("spectral.delay_ms", self.spectral.delay_ms, &mut issues);
            if !self.spectral.window_ms.is_finite() || self.spectral.window_ms <= 0.0 {
                issues.push("spectral.window_ms must be finite and greater than zero".to_owned());
            }
            if !self.spectral.minimum_score.is_finite()
                || !(0.0..=1.0).contains(&self.spectral.minimum_score)
            {
                issues.push("spectral.minimum_score must be between 0 and 1".to_owned());
            }
            if !self.spectral.minimum_margin.is_finite()
                || !(0.0..=1.0).contains(&self.spectral.minimum_margin)
            {
                issues.push("spectral.minimum_margin must be between 0 and 1".to_owned());
            }
            if !advanced.fft_size.is_power_of_two() {
                issues.push("advanced.fft_size must be a power of two".to_owned());
            }

            if self.audio.sample_rate > 0
                && self.spectral.window_ms.is_finite()
                && self.spectral.window_ms > 0.0
            {
                let window_samples =
                    (self.audio.sample_rate as f32 * self.spectral.window_ms / 1000.0).round();
                if !window_samples.is_finite() || window_samples <= 1.0 {
                    issues.push("spectral.window_ms must span more than one sample".to_owned());
                } else if window_samples as f64 > advanced.fft_size as f64 {
                    issues.push(
                        "advanced.fft_size must be at least the spectral window length".to_owned(),
                    );
                }
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigValidationError { issues })
        }
    }
}

/// Validation failures, retained as separate messages for UI presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    issues: Vec<String>,
}

impl ConfigValidationError {
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.issues.join("; "))
    }
}

impl Error for ConfigValidationError {}

/// Error encountered while reading, parsing, validating, or saving settings.
#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    Validation {
        path: PathBuf,
        source: ConfigValidationError,
    },
    Serialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    Write {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "cannot read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(
                    formatter,
                    "invalid config JSON {}: {source}",
                    path.display()
                )
            }
            Self::Validation { path, source } => {
                write!(formatter, "invalid config {}: {source}", path.display())
            }
            Self::Serialize { path, source } => {
                write!(
                    formatter,
                    "cannot serialize config {}: {source}",
                    path.display()
                )
            }
            Self::Write { path, source } => {
                write!(formatter, "cannot save config {}: {source}", path.display())
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Parse { source, .. } | Self::Serialize { source, .. } => Some(source),
            Self::Validation { source, .. } => Some(source),
        }
    }
}

/// Result of loading with safe defaults on unreadable or invalid files.
///
/// A missing file is a normal first-launch case and yields `error: None`.
/// Other load errors are exposed while `config` contains safe defaults.
#[derive(Debug)]
pub struct ConfigLoadResult {
    pub config: AppConfig,
    pub error: Option<ConfigError>,
}

/// Resolved default settings path (`$HOME/.config/pss2midi/config.json`).
pub fn default_config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    config_path_for_home(&home)
}

fn config_path_for_home(home: &Path) -> PathBuf {
    home.join(".config/pss2midi/config.json")
}

/// Load settings from the standard config path. A missing file yields defaults.
pub fn load_config() -> Result<AppConfig, ConfigError> {
    load_config_from(default_config_path())
}

/// Load settings from a path. A missing file yields defaults; invalid files
/// return an error so callers can report the problem.
pub fn load_config_from(path: impl AsRef<Path>) -> Result<AppConfig, ConfigError> {
    let path = path.as_ref().to_path_buf();
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(AppConfig::default());
        }
        Err(source) => return Err(ConfigError::Read { path, source }),
    };

    let config: AppConfig =
        serde_json::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.clone(),
            source,
        })?;
    config
        .validate()
        .map_err(|source| ConfigError::Validation { path, source })?;
    Ok(config)
}

/// Load settings using defaults when the file is absent or cannot be used.
/// Any non-missing-file error remains available to the caller.
pub fn load_config_or_default() -> ConfigLoadResult {
    load_config_or_default_from(default_config_path())
}

/// Path-specific variant of [`load_config_or_default`].
pub fn load_config_or_default_from(path: impl AsRef<Path>) -> ConfigLoadResult {
    match load_config_from(path) {
        Ok(config) => ConfigLoadResult {
            config,
            error: None,
        },
        Err(error) => ConfigLoadResult {
            config: AppConfig::default(),
            error: Some(error),
        },
    }
}

/// Validate and atomically save settings to the standard config path.
pub fn save_config(config: &AppConfig) -> Result<(), ConfigError> {
    save_config_to(config, default_config_path())
}

/// Validate and atomically save settings to a path.
pub fn save_config_to(config: &AppConfig, path: impl AsRef<Path>) -> Result<(), ConfigError> {
    let path = path.as_ref().to_path_buf();
    config
        .validate()
        .map_err(|source| ConfigError::Validation {
            path: path.clone(),
            source,
        })?;

    let mut contents =
        serde_json::to_vec_pretty(config).map_err(|source| ConfigError::Serialize {
            path: path.clone(),
            source,
        })?;
    contents.push(b'\n');
    write_atomically(&path, &contents).map_err(|source| ConfigError::Write { path, source })
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config path has no file name")
    })?;

    fs::create_dir_all(parent)?;
    for _ in 0..128 {
        let id = NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let temporary_name = format!(
            ".{}.{}.{}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
            id
        );
        let temporary_path = parent.join(temporary_name);
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };

        let write_result = {
            let mut file = file;
            file.write_all(contents).and_then(|()| file.sync_all())
        };
        let result = write_result.and_then(|()| fs::rename(&temporary_path, path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        return result;
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique temporary config file",
    ))
}

fn require_nonnegative_finite(field: &str, value: f32, issues: &mut Vec<String>) {
    if !value.is_finite() || value < 0.0 {
        issues.push(format!("{field} must be finite and nonnegative"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("pss2midi-config-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temporary test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn config_paths_keep_user_settings_and_templates_separate() {
        let config_path = config_path_for_home(Path::new("/home/example"));
        assert_eq!(
            config_path,
            PathBuf::from("/home/example/.config/pss2midi/config.json")
        );
        let expected_default_path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config/pss2midi/config.json");
        assert_eq!(default_config_path(), expected_default_path);
        let template_path = AppConfig::default().template_path;
        assert_eq!(
            template_path.file_name(),
            Some(std::ffi::OsStr::new("pss-f30-templates.json"))
        );
        assert_ne!(default_config_path(), template_path);
    }

    #[test]
    fn defaults_match_existing_engine_values_and_start_stopped() {
        let config = AppConfig::default();

        assert_eq!(
            config.audio,
            AudioSettings {
                device: "pipewire".to_owned(),
                sample_rate: 48_000,
            }
        );
        assert_eq!(config.detector_mode, DetectorMode::Yin);
        assert_eq!(
            config.spectral,
            SpectralSettings {
                delay_ms: 8.0,
                window_ms: 30.0,
                minimum_score: 0.75,
                minimum_margin: 0.03,
            }
        );
        assert_eq!(
            config.advanced,
            AdvancedSettings {
                hop_size: 128,
                onset_buffer_size: 1024,
                silence_db: -45.0,
                pitch_buffer_size: 2048,
                attack_ignore_ms: 10.0,
                decision_window_ms: 20.0,
                decision_extend_ms: 10.0,
                release_ms: 30.0,
                retrigger_ms: 60.0,
                onset_threshold: 0.30,
                vote_ratio: 0.60,
                initial_stable_frames: 10,
                fft_size: 2048,
            }
        );
        assert!(!config.auto_start);
    }

    #[test]
    fn runtime_mapping_carries_default_and_custom_audio_detector_spectral_and_template_settings() {
        let defaults = AppConfig::default().to_run_args();
        assert_eq!(defaults.audio.device, "pipewire");
        assert_eq!(defaults.audio.sample_rate, 48_000);
        assert_eq!(defaults.audio.hop, 128);
        assert_eq!(defaults.audio.onset_buffer, 1024);
        assert_eq!(defaults.audio.silence_db, -45.0);
        assert_eq!(defaults.detector, DetectorMode::Yin);
        assert_eq!(defaults.pitch_buffer, 2048);
        assert_eq!(defaults.attack_ignore_ms, 10.0);
        assert_eq!(defaults.decision_window_ms, 20.0);
        assert_eq!(defaults.decision_extend_ms, 10.0);
        assert_eq!(defaults.release_ms, 30.0);
        assert_eq!(defaults.retrigger_ms, 60.0);
        assert_eq!(defaults.onset_threshold, 0.30);
        assert_eq!(defaults.vote_ratio, 0.60);
        assert_eq!(defaults.initial_stable, 10);
        assert_eq!(defaults.spectral.spectral_delay_ms, 8.0);
        assert_eq!(defaults.spectral.spectral_window_ms, 30.0);
        assert_eq!(defaults.spectral.fft_size, 2048);
        assert_eq!(defaults.spectral.spectral_min_score, 0.75);
        assert_eq!(defaults.spectral.spectral_min_margin, 0.03);
        assert_eq!(defaults.templates, Some(AppConfig::default().template_path));

        let mut config = AppConfig::default();
        config.audio.device = "hw:4,0".to_owned();
        config.audio.sample_rate = 44_100;
        config.detector_mode = DetectorMode::Compare;
        config.template_path = PathBuf::from("/tmp/custom-templates.json");
        config.spectral.delay_ms = 12.5;
        config.spectral.window_ms = 24.0;
        config.spectral.minimum_score = 0.82;
        config.spectral.minimum_margin = 0.07;
        config.advanced.hop_size = 256;
        config.advanced.onset_buffer_size = 2048;
        config.advanced.silence_db = -38.0;
        config.advanced.pitch_buffer_size = 4096;
        config.advanced.attack_ignore_ms = 15.0;
        config.advanced.decision_window_ms = 25.0;
        config.advanced.decision_extend_ms = 5.0;
        config.advanced.release_ms = 35.0;
        config.advanced.retrigger_ms = 75.0;
        config.advanced.onset_threshold = 0.42;
        config.advanced.vote_ratio = 0.72;
        config.advanced.initial_stable_frames = 12;
        config.advanced.fft_size = 4096;

        let args = config.to_run_args();
        assert_eq!(args.audio.device, "hw:4,0");
        assert_eq!(args.audio.sample_rate, 44_100);
        assert_eq!(args.audio.hop, 256);
        assert_eq!(args.audio.onset_buffer, 2048);
        assert_eq!(args.audio.silence_db, -38.0);
        assert_eq!(args.detector, DetectorMode::Compare);
        assert_eq!(args.pitch_buffer, 4096);
        assert_eq!(args.attack_ignore_ms, 15.0);
        assert_eq!(args.decision_window_ms, 25.0);
        assert_eq!(args.decision_extend_ms, 5.0);
        assert_eq!(args.release_ms, 35.0);
        assert_eq!(args.retrigger_ms, 75.0);
        assert_eq!(args.onset_threshold, 0.42);
        assert_eq!(args.vote_ratio, 0.72);
        assert_eq!(args.initial_stable, 12);
        assert_eq!(args.spectral.spectral_delay_ms, 12.5);
        assert_eq!(args.spectral.spectral_window_ms, 24.0);
        assert_eq!(args.spectral.spectral_min_score, 0.82);
        assert_eq!(args.spectral.spectral_min_margin, 0.07);
        assert_eq!(args.spectral.fft_size, 4096);
        assert_eq!(
            args.templates,
            Some(PathBuf::from("/tmp/custom-templates.json"))
        );
    }

    #[test]
    fn serde_round_trip_preserves_configured_settings_without_template_contents() {
        let mut config = AppConfig::default();
        config.audio.device = "hw:1,0".to_owned();
        config.detector_mode = DetectorMode::Compare;
        config.template_path = PathBuf::from("/tmp/custom-templates.json");
        config.spectral.minimum_margin = 0.08;
        config.advanced.release_ms = 42.0;
        config.auto_start = true;

        let encoded = serde_json::to_string_pretty(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, config);
        assert!(encoded.contains("\"detector_mode\": \"compare\""));
        assert!(encoded.contains("\"template_path\": \"/tmp/custom-templates.json\""));
        assert!(!encoded.contains("\"notes\""));
    }

    #[test]
    fn missing_config_uses_safe_defaults_without_creating_a_file() {
        let directory = TestDirectory::new();
        let path = directory.path().join("missing/config.json");

        let loaded = load_config_from(&path).unwrap();
        let safe = load_config_or_default_from(&path);

        assert_eq!(loaded, AppConfig::default());
        assert_eq!(safe.config, AppConfig::default());
        assert!(safe.error.is_none());
        assert!(!path.exists());
        assert_eq!(loaded.detector_mode, DetectorMode::Yin);
        assert!(!loaded.auto_start);
    }

    #[test]
    fn invalid_json_and_invalid_values_are_reported_with_safe_defaults_available() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config.json");

        fs::write(&path, "{not json").unwrap();
        assert!(matches!(
            load_config_from(&path),
            Err(ConfigError::Parse { .. })
        ));
        let safe = load_config_or_default_from(&path);
        assert_eq!(safe.config, AppConfig::default());
        assert!(safe.error.is_some());

        fs::write(
            &path,
            r#"{"detector_mode":"spectral","spectral":{"minimum_score":1.5}}"#,
        )
        .unwrap();
        assert!(matches!(
            load_config_from(&path),
            Err(ConfigError::Validation { .. })
        ));
        let safe = load_config_or_default_from(&path);
        assert_eq!(safe.config, AppConfig::default());
        assert!(safe.error.is_some());
    }

    #[test]
    fn validation_enforces_engine_constraints_only_for_active_detector_modes() {
        let defaults = AppConfig::default();
        assert!(defaults.validate().is_ok());

        let mut config = defaults.clone();
        config.audio.sample_rate = 0;
        assert!(config.validate().is_err());

        let mut config = defaults.clone();
        config.advanced.hop_size = 0;
        assert!(config.validate().is_err());

        let mut config = defaults.clone();
        config.spectral.minimum_score = 1.5;
        config.advanced.fft_size = 1000;
        assert!(config.validate().is_ok());
        config.detector_mode = DetectorMode::Spectral;
        assert!(config.validate().is_err());

        let mut config = defaults.clone();
        config.detector_mode = DetectorMode::Spectral;
        config.advanced.pitch_buffer_size = 0;
        config.advanced.vote_ratio = 2.0;
        config.advanced.initial_stable_frames = 0;
        assert!(config.validate().is_ok());
        config.detector_mode = DetectorMode::Compare;
        assert!(config.validate().is_err());

        let mut config = defaults;
        config.advanced.release_ms = -1.0;
        assert!(config.validate().is_err());
        config.advanced.release_ms = 0.0;
        config.advanced.silence_db = 40.0;
        config.advanced.onset_threshold = -0.5;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn spectral_validation_checks_window_fit_and_threshold_ranges() {
        let mut config = AppConfig::default();
        config.detector_mode = DetectorMode::Compare;
        config.advanced.fft_size = 1024;
        config.spectral.window_ms = 30.0;
        assert!(config.validate().is_err());

        config.advanced.fft_size = 2048;
        config.spectral.delay_ms = -1.0;
        assert!(config.validate().is_err());
        config.spectral.delay_ms = 0.0;
        config.spectral.minimum_margin = 1.1;
        assert!(config.validate().is_err());
        config.spectral.minimum_margin = 0.03;
        config.advanced.vote_ratio = 1.01;
        assert!(config.validate().is_err());
    }

    #[test]
    fn atomic_save_replaces_complete_config_and_cleans_temporary_files() {
        let directory = TestDirectory::new();
        let path = directory.path().join("nested/config.json");
        let mut previous = AppConfig::default();
        previous.audio.device = "hw:1,0".to_owned();
        save_config_to(&previous, &path).unwrap();

        let mut config = AppConfig::default();
        config.audio.device = "hw:2,0".to_owned();
        config.auto_start = true;

        save_config_to(&config, &path).unwrap();

        assert_eq!(load_config_from(&path).unwrap(), config);
        let files = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(files, vec![std::ffi::OsString::from("config.json")]);
    }

    #[test]
    fn failed_atomic_replacement_preserves_existing_target_and_cleans_temp_file() {
        let directory = TestDirectory::new();
        let target_directory = directory.path().join("config.json");
        fs::create_dir(&target_directory).unwrap();
        let sentinel = target_directory.join("keep.txt");
        fs::write(&sentinel, "original").unwrap();

        assert!(save_config_to(&AppConfig::default(), &target_directory).is_err());

        assert_eq!(fs::read_to_string(sentinel).unwrap(), "original");
        let entries = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.json")]);
    }
}
