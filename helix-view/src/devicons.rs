//! Optional Nerd Font icons. Configuration belongs to the top-level `[devicons]`
//! table, independently of editor settings and without process-global mutable state.

mod data;

use std::{collections::HashMap, path::Path, sync::LazyLock};

use helix_core::unicode::width::UnicodeWidthStr;
use serde::{Deserialize, Deserializer};

use crate::{
    theme::{Color, Style},
    Theme,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    pub enable: bool,
    pub picker: bool,
    pub bufferline: bool,
    pub breadcrumb: bool,
    pub statusline: bool,
    pub completion: bool,
    pub color: bool,
    pub filename: HashMap<String, IconConfig>,
    pub extension: HashMap<String, IconConfig>,
    pub directory: HashMap<String, IconConfig>,
    pub kind: HashMap<String, IconConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enable: false,
            picker: true,
            bufferline: true,
            breadcrumb: true,
            statusline: true,
            completion: true,
            color: true,
            filename: HashMap::new(),
            extension: HashMap::new(),
            directory: HashMap::new(),
            kind: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum IconConfig {
    Glyph(#[serde(deserialize_with = "deserialize_glyph")] String),
    Styled {
        #[serde(default, deserialize_with = "deserialize_optional_glyph")]
        glyph: Option<String>,
        #[serde(default, deserialize_with = "deserialize_color")]
        fg: Option<Color>,
    },
}

fn deserialize_glyph<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    let glyph = String::deserialize(de)?;
    if glyph
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return Err(serde::de::Error::custom(
            "icon glyphs cannot contain control characters",
        ));
    }
    Ok(glyph)
}

fn deserialize_optional_glyph<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<Option<String>, D::Error> {
    deserialize_glyph(de).map(Some)
}

fn deserialize_color<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Color>, D::Error> {
    let value = String::deserialize(de)?;
    Color::from_hex(&value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon<'a> {
    pub glyph: &'a str,
    pub style: Style,
}

impl Icon<'_> {
    pub fn width(&self) -> usize {
        self.glyph.width()
    }
}

type Defaults = HashMap<&'static str, (&'static str, Option<Color>)>;

fn defaults(data: &'static [(&str, &str, Option<&str>)]) -> Defaults {
    data.iter()
        .map(|&(name, glyph, fg)| {
            (
                name,
                (
                    glyph,
                    fg.map(|hex| Color::from_hex(hex).expect("valid built-in icon color")),
                ),
            )
        })
        .collect()
}

static FILENAMES: LazyLock<Defaults> = LazyLock::new(|| defaults(data::FILENAMES));
static EXTENSIONS: LazyLock<Defaults> = LazyLock::new(|| defaults(data::EXTENSIONS));
static DIRECTORIES: LazyLock<Defaults> = LazyLock::new(|| defaults(data::DIRECTORIES));
static KINDS: LazyLock<Defaults> = LazyLock::new(|| defaults(data::KINDS));

impl Config {
    pub fn file<'a>(
        &'a self,
        path: Option<&Path>,
        language: Option<&str>,
        theme: &Theme,
    ) -> Icon<'a> {
        let filename = path
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy());
        if let Some(name) = filename.as_deref() {
            if self.filename.contains_key(name) || FILENAMES.contains_key(name) {
                return self.resolve(
                    name,
                    &self.filename,
                    &FILENAMES,
                    "",
                    "ui.devicons.file",
                    theme,
                );
            }
            // Prefer the longest recognized suffix, e.g. `blade.php` before `php`.
            for (index, _) in name.match_indices('.') {
                let extension = &name[index + 1..];
                if self.extension.contains_key(extension) || EXTENSIONS.contains_key(extension) {
                    return self.resolve(
                        extension,
                        &self.extension,
                        &EXTENSIONS,
                        "",
                        "ui.devicons.file",
                        theme,
                    );
                }
            }
        }
        // Scratch buffers can still have a detected/explicitly selected language.
        let extension = match language {
            Some("rust") => "rs",
            Some("javascript") => "js",
            Some("typescript") => "ts",
            Some("python") => "py",
            Some("markdown") => "md",
            Some("bash") => "sh",
            Some("c-sharp") => "cs",
            Some("cpp") => "cpp",
            Some(language) => language,
            None => "",
        };
        self.resolve(
            extension,
            &self.extension,
            &EXTENSIONS,
            "",
            "ui.devicons.file",
            theme,
        )
    }

    pub fn directory<'a>(&'a self, path: &Path, theme: &Theme) -> Icon<'a> {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let key = if self.directory.contains_key(name.as_ref())
            || DIRECTORIES.contains_key(name.as_ref())
        {
            name.as_ref()
        } else {
            "default"
        };
        self.resolve(
            key,
            &self.directory,
            &DIRECTORIES,
            "󰉋",
            "ui.devicons.directory",
            theme,
        )
    }

    pub fn kind<'a>(&'a self, kind: &str, theme: &Theme) -> Icon<'a> {
        self.resolve(
            kind,
            &self.kind,
            &KINDS,
            "",
            &format!("ui.devicons.kind.{kind}"),
            theme,
        )
    }

    fn resolve<'a>(
        &'a self,
        key: &str,
        overrides: &'a HashMap<String, IconConfig>,
        defaults: &Defaults,
        fallback: &'static str,
        scope: &str,
        theme: &Theme,
    ) -> Icon<'a> {
        let (mut glyph, mut fg) = defaults.get(key).copied().unwrap_or((fallback, None));
        let mut style = if self.color {
            theme.try_get(scope).unwrap_or_default()
        } else {
            Style::default()
        };
        if self.color && style.fg.is_none() {
            style.fg = fg;
        }
        if let Some(icon) = overrides.get(key) {
            match icon {
                IconConfig::Glyph(value) => glyph = value,
                IconConfig::Styled {
                    glyph: value,
                    fg: color,
                } => {
                    if let Some(value) = value {
                        glyph = value;
                    }
                    fg = *color;
                    if self.color && fg.is_some() {
                        style.fg = fg;
                    }
                }
            }
        }
        Icon { glyph, style }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DEFAULT_THEME;

    #[test]
    fn filename_and_longest_extension_take_precedence() {
        let config: Config = toml::from_str(
            r#"
            [filename]
            "component.blade.php" = "N"
            [extension]
            "blade.php" = "B"
            php = "P"
            brandnew = "X"
        "#,
        )
        .unwrap();
        let icon = |path| {
            config
                .file(Some(Path::new(path)), None, &DEFAULT_THEME)
                .glyph
        };
        assert_eq!(icon("component.blade.php"), "N");
        assert_eq!(icon("other.blade.php"), "B");
        assert_eq!(icon("other.php"), "P");
        assert_eq!(icon("other.brandnew"), "X");
        assert_eq!(icon("Dockerfile"), FILENAMES["Dockerfile"].0);
    }

    #[test]
    fn scratch_unknown_and_custom_directory() {
        let config: Config = toml::from_str("[directory]\nassets = 'A'").unwrap();
        assert_eq!(
            config.file(None, Some("rust"), &DEFAULT_THEME).glyph,
            EXTENSIONS["rs"].0
        );
        assert_eq!(config.file(None, None, &DEFAULT_THEME).glyph, "");
        assert_eq!(
            config.directory(Path::new("assets"), &DEFAULT_THEME).glyph,
            "A"
        );
        assert_eq!(config.kind("unknown", &DEFAULT_THEME).glyph, "");
    }

    #[test]
    fn colors_inherit_and_can_be_disabled() {
        let mut config: Config = toml::from_str("[extension]\nrs = { fg = '#123456' }").unwrap();
        let icon = config.file(Some(Path::new("a.rs")), None, &DEFAULT_THEME);
        assert_eq!(icon.glyph, EXTENSIONS["rs"].0);
        assert_eq!(icon.style.fg, Some(Color::Rgb(0x12, 0x34, 0x56)));
        config.color = false;
        assert_eq!(
            config
                .file(Some(Path::new("a.rs")), None, &DEFAULT_THEME)
                .style,
            Style::default()
        );
    }

    #[test]
    fn glyphs_use_display_width_and_reject_controls() {
        let config: Config = toml::from_str("[extension]\nwide = '界'\nempty = ''").unwrap();
        assert_eq!(
            config
                .file(Some(Path::new("a.wide")), None, &DEFAULT_THEME)
                .width(),
            2
        );
        assert_eq!(
            config
                .file(Some(Path::new("a.empty")), None, &DEFAULT_THEME)
                .width(),
            0
        );
        for value in [
            "[extension]\nrs = \"a\\nb\"",
            "[extension]\nrs = { fg = 'invalid' }",
            "bogus = true",
        ] {
            assert!(toml::from_str::<Config>(value).is_err(), "{value}");
        }
    }
}
