//! Colour-scheme preference. `Auto` follows the OS; the other two pin it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn from_key(raw: &str) -> Self {
        match raw {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::Auto,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::Auto,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Light => "sun",
            Self::Dark => "moon",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Theme: follows your system",
            Self::Light => "Theme: light",
            Self::Dark => "Theme: dark",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Theme;

    #[test]
    fn unknown_keys_fall_back_to_auto() {
        assert_eq!(Theme::from_key(""), Theme::Auto);
        assert_eq!(Theme::from_key("nonsense"), Theme::Auto);
        assert_eq!(Theme::from_key("dark"), Theme::Dark);
    }

    #[test]
    fn cycles_through_every_mode() {
        let mut seen = Vec::new();
        let mut theme = Theme::Auto;
        for _ in 0..3 {
            seen.push(theme.key());
            theme = theme.next();
        }
        assert_eq!(seen, vec!["auto", "light", "dark"]);
        assert_eq!(theme, Theme::Auto);
    }
}
