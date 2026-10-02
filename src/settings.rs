use std::{fs, path::PathBuf};

pub const FONTS: [&str; 4] = ["Geist Mono", "JetBrains Mono", "Fira Code", "monospace"];
pub const RAIL_ITEMS: [&str; 6] = ["files", "search", "git", "trello", "opencode", "terminal"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Darker,
    Classic,
    Gere,
}

pub struct Settings {
    pub theme: Theme,
    pub font: usize,
    pub font_size: u8,
    pub git_font_size: u8,
    pub blame_delay_ms: u16,
    pub rail_order: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Darker,
            font: 0,
            font_size: 14,
            git_font_size: 12,
            blame_delay_ms: 500,
            rail_order: RAIL_ITEMS.iter().map(|name| (*name).into()).collect(),
        }
    }
}

impl Settings {
    pub fn is_gere(&self) -> bool {
        self.theme == Theme::Gere
    }

    pub fn font_name(&self) -> &'static str {
        FONTS[self.font]
    }

    pub fn background(&self) -> u32 {
        match self.theme {
            Theme::Darker => 0x21252b,
            Theme::Classic => 0x282c34,
            Theme::Gere => 0x141920,
        }
    }

    pub fn panel(&self) -> u32 {
        match self.theme {
            Theme::Darker => 0x282c34,
            Theme::Classic => 0x2c313a,
            Theme::Gere => 0x161b23,
        }
    }

    fn path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|dir| dir.join("gere").join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        Self::from_value(&value)
    }

    fn from_value(value: &serde_json::Value) -> Self {
        Self {
            theme: match value["theme"].as_str() {
                Some("Gere Theme") => Theme::Gere,
                Some("One Dark Pro") => Theme::Classic,
                _ => Theme::Darker,
            },
            font: value["font"]
                .as_str()
                .and_then(|name| FONTS.iter().position(|font| font == &name))
                .unwrap_or(0),
            font_size: value["font_size"].as_u64().unwrap_or(14).clamp(10, 24) as u8,
            git_font_size: value["git_font_size"].as_u64().unwrap_or(12).clamp(10, 16) as u8,
            blame_delay_ms: value["blame_delay_ms"].as_u64().unwrap_or(500).min(2000) as u16,
            rail_order: {
                let mut order = Vec::new();
                for item in value["rail_order"].as_array().into_iter().flatten() {
                    if let Some(name) = item.as_str() {
                        if RAIL_ITEMS.contains(&name) && !order.iter().any(|entry| entry == name) {
                            order.push(name.to_owned());
                        }
                    }
                }
                for name in RAIL_ITEMS {
                    if !order.iter().any(|entry| entry == name) {
                        order.push(name.into());
                    }
                }
                order
            },
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = Self::path() else {
            return Ok(());
        };
        fs::create_dir_all(path.parent().expect("settings parent"))?;
        fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "theme": match self.theme {
                    Theme::Darker => "One Dark Pro Darker",
                    Theme::Classic => "One Dark Pro",
                    Theme::Gere => "Gere Theme",
                },
                "font": self.font_name(),
                "font_size": self.font_size,
                "git_font_size": self.git_font_size,
                "blame_delay_ms": self.blame_delay_ms,
                "rail_order": self.rail_order,
            }))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blame_delay_defaults_for_existing_preferences_and_clamps_invalid_values() {
        assert_eq!(Settings::default().blame_delay_ms, 500);
        assert_eq!(
            Settings::from_value(&serde_json::json!({"font_size": 14})).blame_delay_ms,
            500
        );
        assert_eq!(
            Settings::from_value(&serde_json::json!({"blame_delay_ms": 0})).blame_delay_ms,
            0
        );
        assert_eq!(
            Settings::from_value(&serde_json::json!({"blame_delay_ms": 9999})).blame_delay_ms,
            2000
        );
    }

    #[test]
    fn theme_names_keep_existing_preferences_and_restore_gere() {
        assert!(
            Settings::from_value(&serde_json::json!({"theme": "One Dark Pro"})).theme
                == Theme::Classic
        );
        assert!(
            Settings::from_value(&serde_json::json!({"theme": "One Dark Pro Darker"})).theme
                == Theme::Darker
        );
        assert!(
            Settings::from_value(&serde_json::json!({"theme": "Gere Theme"})).theme == Theme::Gere
        );
    }

    #[test]
    fn rail_order_recovers_missing_and_duplicate_panels() {
        let settings = Settings::from_value(&serde_json::json!({
            "rail_order": ["terminal", "trello", "terminal", "unknown", "files"]
        }));
        assert_eq!(
            settings.rail_order,
            ["terminal", "trello", "files", "search", "git", "opencode"]
        );
        assert_eq!(Settings::default().rail_order, RAIL_ITEMS);
    }
}
