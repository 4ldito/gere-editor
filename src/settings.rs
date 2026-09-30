use std::{fs, path::PathBuf};

const FONTS: [&str; 4] = ["Geist Mono", "JetBrains Mono", "Fira Code", "monospace"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Darker,
    Classic,
}

pub struct Settings {
    pub theme: Theme,
    pub font: usize,
    pub font_size: u8,
    pub git_font_size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Darker,
            font: 0,
            font_size: 14,
            git_font_size: 12,
        }
    }
}

impl Settings {
    pub fn font_name(&self) -> &'static str {
        FONTS[self.font]
    }

    pub fn next_font(&mut self) {
        self.font = (self.font + 1) % FONTS.len();
    }

    pub fn background(&self) -> u32 {
        match self.theme {
            Theme::Darker => 0x21252b,
            Theme::Classic => 0x282c34,
        }
    }

    pub fn panel(&self) -> u32 {
        match self.theme {
            Theme::Darker => 0x282c34,
            Theme::Classic => 0x2c313a,
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
        Self {
            theme: if value["theme"] == "One Dark Pro" {
                Theme::Classic
            } else {
                Theme::Darker
            },
            font: value["font"]
                .as_str()
                .and_then(|name| FONTS.iter().position(|font| font == &name))
                .unwrap_or(0),
            font_size: value["font_size"].as_u64().unwrap_or(14).clamp(10, 24) as u8,
            git_font_size: value["git_font_size"].as_u64().unwrap_or(12).clamp(10, 16) as u8,
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
                "theme": if self.theme == Theme::Darker { "One Dark Pro Darker" } else { "One Dark Pro" },
                "font": self.font_name(),
                "font_size": self.font_size,
                "git_font_size": self.git_font_size,
            }))?,
        )
    }
}
