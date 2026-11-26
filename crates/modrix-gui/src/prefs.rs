// SPDX-License-Identifier: GPL-2.0-only
//! The GUI's own preferences, in `<config>/gui.toml`. Read once at start and
//! written only when the user changes one, never during a frame.

use modrix_core::Paths;

/// Light, dark, or follow the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    /// Follow the desktop.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

impl Appearance {
    /// In picker order.
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Dark, Appearance::Light];

    /// Picker label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Dark => "dark",
            Appearance::Light => "light",
        }
    }

    /// Parse the stored file.
    #[must_use]
    pub fn parse(text: &str) -> Appearance {
        let value = text
            .lines()
            .take(64)
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim() == "appearance")
            .map(|(_, v)| v.trim().trim_matches('"'));
        match value {
            Some("dark") => Appearance::Dark,
            Some("light") => Appearance::Light,
            _ => Appearance::System,
        }
    }
}

/// The stored appearance (System when absent or unreadable).
#[must_use]
pub fn load() -> Appearance {
    let Ok(paths) = Paths::resolve() else {
        return Appearance::System;
    };
    std::fs::read_to_string(paths.config_dir().join("gui.toml"))
        .map(|t| Appearance::parse(&t))
        .unwrap_or_default()
}

/// Store the appearance.
///
/// # Errors
/// When the config directory cannot be written.
pub fn save(paths: &Paths, appearance: Appearance) -> std::io::Result<()> {
    let dir = paths.config_dir();
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("gui.toml"),
        format!("appearance = \"{}\"\n", appearance.id()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        assert_eq!(
            Appearance::parse("appearance = \"dark\"\n"),
            Appearance::Dark
        );
        assert_eq!(Appearance::parse("theme = \"x\"\n"), Appearance::System);
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted_at(tmp.path());
        save(&paths, Appearance::Light).unwrap();
        let text = std::fs::read_to_string(paths.config_dir().join("gui.toml")).unwrap();
        assert_eq!(Appearance::parse(&text), Appearance::Light);
    }
}
