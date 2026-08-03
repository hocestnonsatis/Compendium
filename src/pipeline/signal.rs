//! Signal-to-call threshold: skip compression on tiny payloads to avoid inflation.

use crate::config::Config;

/// Whether the pipeline should bypass compression/summarization for this input.
pub fn should_bypass_signal(input: &str, config: &Config, force: bool) -> bool {
    if force {
        return false;
    }
    let min = config.signal_min_chars;
    if min == 0 {
        return false;
    }
    input.chars().count() < min
}

/// Human-readable bypass reason for metrics / results.
pub fn bypass_reason(config: &Config) -> String {
    format!(
        "input shorter than signal threshold ({} chars); skipped to avoid wrapper inflation",
        config.signal_min_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypasses_short_input() {
        let cfg = Config {
            signal_min_chars: 1000,
            ..Config::default()
        };
        assert!(should_bypass_signal("short", &cfg, false));
        assert!(!should_bypass_signal("short", &cfg, true));
    }

    #[test]
    fn processes_long_input() {
        let cfg = Config::default();
        let long: String = "x".repeat(1001);
        assert!(!should_bypass_signal(&long, &cfg, false));
    }
}
