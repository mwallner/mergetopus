use std::cell::RefCell;
use std::io::IsTerminal;

/// Determines whether colored output should be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Auto-detect based on terminal capability and environment.
    Auto,
    /// Always use colors regardless of terminal support.
    Always,
    /// Never use colors.
    Never,
}

impl std::str::FromStr for ColorMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(ColorMode::Auto),
            "always" => Ok(ColorMode::Always),
            "never" => Ok(ColorMode::Never),
            _ => Err(format!(
                "invalid color mode '{}'; use 'auto', 'always', or 'never'",
                s
            )),
        }
    }
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColorMode::Auto => write!(f, "auto"),
            ColorMode::Always => write!(f, "always"),
            ColorMode::Never => write!(f, "never"),
        }
    }
}

/// Runtime configuration for colored output.
#[derive(Debug, Clone, Copy)]
pub struct ColorConfig {
    /// Whether to enable colored output.
    pub enabled: bool,
}

impl ColorConfig {
    /// Create a new ColorConfig based on the given mode.
    pub fn new(mode: ColorMode) -> Self {
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                // Auto-detect: colors enabled if stdout is a terminal
                std::io::stdout().is_terminal()
            }
        };
        Self { enabled }
    }

    /// Get the default ColorConfig (Auto mode).
    pub fn default_auto() -> Self {
        Self::new(ColorMode::Auto)
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self::default_auto()
    }
}

thread_local! {
    static COLOR_CONFIG: RefCell<ColorConfig> = RefCell::new(ColorConfig::default());
}

/// Initialize the global color configuration.
/// This should be called once at startup in main().
pub fn init_config(config: ColorConfig) {
    COLOR_CONFIG.with(|cfg| {
        *cfg.borrow_mut() = config;
    });
}

/// Get the current global color configuration.
/// Primarily for internal use; prefer the typed helper functions below.
pub fn get_config(override_config: Option<&ColorConfig>) -> ColorConfig {
    override_config.copied().unwrap_or_else(|| {
        COLOR_CONFIG.with(|cfg| *cfg.borrow())
    })
}

// ============================================================================
// Helper functions for colored output
// ============================================================================

use owo_colors::OwoColorize;

/// Print a success message in green.
#[inline]
pub fn print_success(msg: &str, config: Option<&ColorConfig>) {
    let cfg = get_config(config);
    if cfg.enabled {
        println!("{}", msg.green());
    } else {
        println!("{}", msg);
    }
}

/// Print an error message in red to stderr.
#[inline]
pub fn print_error(msg: &str, config: Option<&ColorConfig>) {
    let cfg = get_config(config);
    if cfg.enabled {
        eprintln!("{}", msg.red());
    } else {
        eprintln!("{}", msg);
    }
}

/// Print a warning message in yellow.
#[inline]
pub fn print_warning(msg: &str, config: Option<&ColorConfig>) {
    let cfg = get_config(config);
    if cfg.enabled {
        println!("{}", msg.yellow());
    } else {
        println!("{}", msg);
    }
}

/// Print an info message in a neutral color (dim).
#[inline]
pub fn print_info(msg: &str, config: Option<&ColorConfig>) {
    let cfg = get_config(config);
    if cfg.enabled {
        println!("{}", msg.dimmed());
    } else {
        println!("{}", msg);
    }
}

/// Print an emphasized message in bold.
#[inline]
pub fn print_emphasis(msg: &str, config: Option<&ColorConfig>) {
    let cfg = get_config(config);
    if cfg.enabled {
        println!("{}", msg.bold());
    } else {
        println!("{}", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_mode_parse() {
        assert_eq!("auto".parse::<ColorMode>().unwrap(), ColorMode::Auto);
        assert_eq!("always".parse::<ColorMode>().unwrap(), ColorMode::Always);
        assert_eq!("never".parse::<ColorMode>().unwrap(), ColorMode::Never);
        assert!("invalid".parse::<ColorMode>().is_err());
    }

    #[test]
    fn test_color_config_always() {
        let cfg = ColorConfig::new(ColorMode::Always);
        assert!(cfg.enabled);
    }

    #[test]
    fn test_color_config_never() {
        let cfg = ColorConfig::new(ColorMode::Never);
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_override_config() {
        let default_cfg = ColorConfig::new(ColorMode::Never);
        let override_cfg = ColorConfig::new(ColorMode::Always);

        // When override is provided, it should be used
        let result = get_config(Some(&override_cfg));
        assert!(result.enabled);

        // When override is None, it should use the thread-local (which is the default)
        let result = get_config(None);
        assert!(!result.enabled); // default is Auto, but we're not in a terminal during tests
    }
}
