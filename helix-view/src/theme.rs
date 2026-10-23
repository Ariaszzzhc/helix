use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str,
};

use anyhow::{anyhow, Result};
use helix_core::{hashmap, syntax::Highlight};
use helix_loader::merge_toml_values;
use log::warn;
use serde::{Deserialize, Deserializer};
use std::sync::LazyLock;
use toml::{map::Map, Value};

use crate::graphics::UnderlineStyle;
pub use crate::graphics::{Color, Modifier, Style};

pub static DEFAULT_THEME_DATA: LazyLock<Value> = LazyLock::new(|| {
    let bytes = include_bytes!("../../theme.toml");
    toml::from_str(str::from_utf8(bytes).unwrap()).expect("Failed to parse base default theme")
});

pub static BASE16_DEFAULT_THEME_DATA: LazyLock<Value> = LazyLock::new(|| {
    let bytes = include_bytes!("../../base16_theme.toml");
    toml::from_str(str::from_utf8(bytes).unwrap()).expect("Failed to parse base 16 default theme")
});

pub static DEFAULT_THEME: LazyLock<Theme> = LazyLock::new(|| Theme {
    name: "default".into(),
    ..Theme::from(DEFAULT_THEME_DATA.clone())
});

pub static BASE16_DEFAULT_THEME: LazyLock<Theme> = LazyLock::new(|| Theme {
    name: "base16_default".into(),
    ..Theme::from(BASE16_DEFAULT_THEME_DATA.clone())
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    Dark,
    Light,
}

#[cfg(feature = "term")]
impl From<termina::escape::csi::ThemeMode> for Mode {
    fn from(mode: termina::escape::csi::ThemeMode) -> Self {
        match mode {
            termina::escape::csi::ThemeMode::Dark => Self::Dark,
            termina::escape::csi::ThemeMode::Light => Self::Light,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged, deny_unknown_fields, rename_all = "kebab-case")]
pub enum Config {
    Constant(String),
    Adaptive {
        light: String,
        dark: String,
        /// A theme to choose when the terminal did not declare either light or dark mode.
        /// When not specified the dark theme is preferred.
        fallback: Option<String>,
    },
}

impl Config {
    pub fn choose(&self, preference: Option<Mode>) -> &str {
        match self {
            Config::Constant(theme) => theme,
            Config::Adaptive {
                light,
                dark,
                fallback,
            } => match preference {
                Some(Mode::Light) => light,
                Some(Mode::Dark) => dark,
                None => fallback.as_ref().unwrap_or(dark),
            },
        }
    }

    pub fn is_adaptive(&self) -> bool {
        matches!(self, Self::Adaptive { .. })
    }
}

#[derive(Clone, Debug)]
pub struct Loader {
    /// Theme directories to search from highest to lowest priority
    theme_dirs: Vec<PathBuf>,
}
impl Loader {
    /// Creates a new loader that can load themes from multiple directories.
    ///
    /// The provided directories should be ordered from highest to lowest priority.
    /// The directories will have their "themes" subdirectory searched.
    pub fn new(dirs: &[PathBuf]) -> Self {
        Self {
            theme_dirs: dirs.iter().map(|p| p.join("themes")).collect(),
        }
    }

    /// Loads a theme searching directories in priority order.
    pub fn load(&self, name: &str) -> Result<Theme> {
        let (theme, warnings) = self.load_with_warnings(name)?;

        for warning in warnings {
            warn!("Theme '{}': {}", name, warning);
        }

        Ok(theme)
    }

    /// Loads a theme searching directories in priority order, returning any warnings
    pub fn load_with_warnings(&self, name: &str) -> Result<(Theme, Vec<String>)> {
        if name == "default" {
            return Ok((self.default(), Vec::new()));
        }
        if name == "base16_default" {
            return Ok((self.base16_default(), Vec::new()));
        }

        let mut visited_paths = HashSet::new();
        let (theme, warnings) = self
            .load_theme(name, &mut visited_paths)
            .map(Theme::from_toml)?;

        let theme = Theme {
            name: name.into(),
            ..theme
        };
        Ok((theme, warnings))
    }

    /// Recursively load a theme, merging with any inherited parent themes.
    ///
    /// The paths that have been visited in the inheritance hierarchy are tracked
    /// to detect and avoid cycling.
    ///
    /// It is possible for one file to inherit from another file with the same name
    /// so long as the second file is in a themes directory with lower priority.
    /// However, it is not recommended that users do this as it will make tracing
    /// errors more difficult.
    fn load_theme(&self, name: &str, visited_paths: &mut HashSet<PathBuf>) -> Result<Value> {
        let path = self.path(name, visited_paths)?;

        let theme_toml = self.load_file(&path)?;

        let inherits = theme_toml.get("inherits");

        let theme_toml = if let Some(parent_theme_name) = inherits {
            let parent_theme_name = parent_theme_name.as_str().ok_or_else(|| {
                anyhow!("Expected 'inherits' to be a string: {}", parent_theme_name)
            })?;

            let parent_theme_toml = match parent_theme_name {
                // load default themes's toml from const.
                "default" => DEFAULT_THEME_DATA.clone(),
                "base16_default" => BASE16_DEFAULT_THEME_DATA.clone(),
                _ => self.load_theme(parent_theme_name, visited_paths)?,
            };

            self.merge_themes(parent_theme_toml, theme_toml)
        } else {
            theme_toml
        };

        Ok(theme_toml)
    }

    pub fn read_names(path: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        let path = entry.path();
                        if !path.is_file()
                            || !matches!(path.extension()?.to_str()?, "toml" | "json" | "jsonc")
                        {
                            return None;
                        }
                        Some(path.file_stem()?.to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort_unstable();
        names.dedup();
        names
    }

    // merge one theme into the parent theme
    fn merge_themes(&self, parent_theme_toml: Value, theme_toml: Value) -> Value {
        let parent_palette = parent_theme_toml.get("palette");
        let palette = theme_toml.get("palette");

        // handle the table separately since it needs a `merge_depth` of 2
        // this would conflict with the rest of the theme merge strategy
        let palette_values = match (parent_palette, palette) {
            (Some(parent_palette), Some(palette)) => {
                merge_toml_values(parent_palette.clone(), palette.clone(), 2)
            }
            (Some(parent_palette), None) => parent_palette.clone(),
            (None, Some(palette)) => palette.clone(),
            (None, None) => Map::new().into(),
        };

        // add the palette correctly as nested table
        let mut palette = Map::new();
        palette.insert(String::from("palette"), palette_values);

        // merge the theme into the parent theme
        let theme = merge_toml_values(parent_theme_toml, theme_toml, 1);
        // merge the before specially handled palette into the theme
        merge_toml_values(theme, palette.into(), 1)
    }

    // Normalize all supported theme formats to the existing merge representation.
    fn load_file(&self, path: &Path) -> Result<Value> {
        let data = std::fs::read_to_string(path)?;
        Ok(helix_loader::config::parse(&data, path)?)
    }

    /// Returns the path to the theme with the given name
    ///
    /// Ignores paths already visited and follows directory priority order.
    /// Within each directory, TOML takes priority over JSON and JSONC. Visiting
    /// a theme skips that directory entirely on subsequent same-name lookups,
    /// so inheritance continues in a lower-priority directory as before.
    fn path(&self, name: &str, visited_paths: &mut HashSet<PathBuf>) -> Result<PathBuf> {
        let mut cycle_found = false; // track if there was a path, but it was in a cycle
        self.theme_dirs
            .iter()
            .find_map(|dir| {
                let path = helix_loader::config::find_config_file(dir, name);
                if !path.exists() {
                    None
                } else if visited_paths.contains(&path) {
                    // Avoiding cycle, continuing to look in lower priority directories
                    cycle_found = true;
                    None
                } else {
                    visited_paths.insert(path.clone());
                    Some(path)
                }
            })
            .ok_or_else(|| {
                if cycle_found {
                    anyhow!("Cycle found in inheriting: {}", name)
                } else {
                    anyhow!("File not found for: {}", name)
                }
            })
    }

    pub fn default_theme(&self, true_color: bool) -> Theme {
        if true_color {
            self.default()
        } else {
            self.base16_default()
        }
    }

    /// Returns the default theme
    pub fn default(&self) -> Theme {
        DEFAULT_THEME.clone()
    }

    /// Returns the alternative 16-color default theme
    pub fn base16_default(&self) -> Theme {
        BASE16_DEFAULT_THEME.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Theme {
    name: String,

    // UI styles are stored in a HashMap
    styles: HashMap<String, Style>,
    // tree-sitter highlight styles are stored in a Vec to optimize lookups
    scopes: Vec<String>,
    highlights: Vec<Style>,
    /// Reverse map from scope string to its `Highlight` index. `find_highlight_exact`
    /// is called many times per frame, so we optimize lookups.
    scope_index: HashMap<String, Highlight>,
    rainbow_length: usize,
}

impl From<Value> for Theme {
    fn from(value: Value) -> Self {
        let (theme, warnings) = Theme::from_toml(value);
        for warning in warnings {
            warn!("{}", warning);
        }
        theme
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Map::<String, Value>::deserialize(deserializer)?;
        let (theme, warnings) = Theme::from_keys(values);
        for warning in warnings {
            warn!("{}", warning);
        }
        Ok(theme)
    }
}

#[allow(clippy::type_complexity)]
fn build_theme_values(
    mut values: Map<String, Value>,
) -> (
    HashMap<String, Style>,
    Vec<String>,
    Vec<Style>,
    usize,
    Vec<String>,
) {
    let mut styles = HashMap::new();
    let mut scopes = Vec::new();
    let mut highlights = Vec::new();
    let mut rainbow_length = 0;

    let mut warnings = Vec::new();

    // TODO: alert user of parsing failures in editor
    let palette = values
        .remove("palette")
        .map(|value| {
            ThemePalette::try_from(value).unwrap_or_else(|err| {
                warnings.push(err);
                ThemePalette::default()
            })
        })
        .unwrap_or_default();
    // remove inherits from value to prevent errors
    let _ = values.remove("inherits");
    styles.reserve(values.len());
    scopes.reserve(values.len());
    highlights.reserve(values.len());

    for (i, style) in values
        .remove("rainbow")
        .and_then(|value| match palette.parse_style_array(value) {
            Ok(styles) => Some(styles),
            Err(err) => {
                warnings.push(err);
                None
            }
        })
        .unwrap_or_else(default_rainbow)
        .into_iter()
        .enumerate()
    {
        let name = format!("rainbow.{i}");
        styles.insert(name.clone(), style);
        scopes.push(name);
        highlights.push(style);
        rainbow_length += 1;
    }

    for (name, style_value) in values {
        let mut style = Style::default();
        if let Err(err) = palette.parse_style(&mut style, style_value) {
            warnings.push(format!("Failed to parse style for key {name:?}. {err}"));
        }

        // these are used both as UI and as highlights
        styles.insert(name.clone(), style);
        scopes.push(name);
        highlights.push(style);
    }

    (styles, scopes, highlights, rainbow_length, warnings)
}

fn default_rainbow() -> Vec<Style> {
    vec![
        Style::default().fg(Color::Red),
        Style::default().fg(Color::Yellow),
        Style::default().fg(Color::Green),
        Style::default().fg(Color::Blue),
        Style::default().fg(Color::Cyan),
        Style::default().fg(Color::Magenta),
    ]
}
impl Theme {
    /// To allow `Highlight` to represent arbitrary RGB colors without turning it into an enum,
    /// we interpret the last 256^3 numbers as RGB.
    const RGB_START: u32 = (u32::MAX << (8 + 8 + 8)) - 1 - (u32::MAX - Highlight::MAX);

    /// Interpret a Highlight with the RGB foreground
    fn decode_rgb_highlight(highlight: Highlight) -> Option<(u8, u8, u8)> {
        (highlight.get() > Self::RGB_START).then(|| {
            let [b, g, r, ..] = (highlight.get() + 1).to_le_bytes();
            (r, g, b)
        })
    }

    /// Create a Highlight that represents an RGB color
    pub fn rgb_highlight(r: u8, g: u8, b: u8) -> Highlight {
        // -1 because highlight is "non-max": u32::MAX is reserved for the null pointer
        // optimization.
        Highlight::new(u32::from_le_bytes([b, g, r, u8::MAX]) - 1)
    }

    #[inline]
    pub fn highlight(&self, highlight: Highlight) -> Style {
        if let Some((red, green, blue)) = Self::decode_rgb_highlight(highlight) {
            Style::new().fg(Color::Rgb(red, green, blue))
        } else {
            self.highlights[highlight.idx()]
        }
    }

    #[inline]
    pub fn scope(&self, highlight: Highlight) -> &str {
        &self.scopes[highlight.idx()]
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn get(&self, scope: &str) -> Style {
        self.try_get(scope).unwrap_or_default()
    }

    /// Get the style of a scope, falling back to dot separated broader
    /// scopes. For example if `ui.text.focus` is not defined in the theme,
    /// `ui.text` is tried and then `ui` is tried.
    pub fn try_get(&self, scope: &str) -> Option<Style> {
        std::iter::successors(Some(scope), |s| Some(s.rsplit_once('.')?.0))
            .find_map(|s| self.styles.get(s).copied())
    }

    /// Get the style of a scope, without falling back to dot separated broader
    /// scopes. For example if `ui.text.focus` is not defined in the theme, it
    /// will return `None`, even if `ui.text` is.
    pub fn try_get_exact(&self, scope: &str) -> Option<Style> {
        self.styles.get(scope).copied()
    }

    #[inline]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    pub fn find_highlight_exact(&self, scope: &str) -> Option<Highlight> {
        self.scope_index.get(scope).copied()
    }

    pub fn find_highlight(&self, mut scope: &str) -> Option<Highlight> {
        loop {
            if let Some(highlight) = self.find_highlight_exact(scope) {
                return Some(highlight);
            }
            if let Some(new_end) = scope.rfind('.') {
                scope = &scope[..new_end];
            } else {
                return None;
            }
        }
    }

    pub fn is_16_color(&self) -> bool {
        self.styles.iter().all(|(_, style)| {
            [style.fg, style.bg]
                .into_iter()
                .all(|color| !matches!(color, Some(Color::Rgb(..))))
        })
    }

    pub fn rainbow_length(&self) -> usize {
        self.rainbow_length
    }

    fn from_toml(value: Value) -> (Self, Vec<String>) {
        if let Value::Table(table) = value {
            Theme::from_keys(table)
        } else {
            warn!("Expected theme TOML value to be a table, found {:?}", value);
            Default::default()
        }
    }

    fn from_keys(toml_keys: Map<String, Value>) -> (Self, Vec<String>) {
        let (styles, scopes, highlights, rainbow_length, load_errors) =
            build_theme_values(toml_keys);

        let scope_index = scopes
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), Highlight::new(i as u32)))
            .collect();

        let theme = Self {
            styles,
            scopes,
            highlights,
            scope_index,
            rainbow_length,
            ..Default::default()
        };
        (theme, load_errors)
    }
}

struct ThemePalette {
    palette: HashMap<String, Color>,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            palette: hashmap! {
                "default".to_string() => Color::Reset,
                "black".to_string() => Color::Black,
                "red".to_string() => Color::Red,
                "green".to_string() => Color::Green,
                "yellow".to_string() => Color::Yellow,
                "blue".to_string() => Color::Blue,
                "magenta".to_string() => Color::Magenta,
                "cyan".to_string() => Color::Cyan,
                "gray".to_string() => Color::Gray,
                "light-red".to_string() => Color::LightRed,
                "light-green".to_string() => Color::LightGreen,
                "light-yellow".to_string() => Color::LightYellow,
                "light-blue".to_string() => Color::LightBlue,
                "light-magenta".to_string() => Color::LightMagenta,
                "light-cyan".to_string() => Color::LightCyan,
                "light-gray".to_string() => Color::LightGray,
                "white".to_string() => Color::White,
            },
        }
    }
}

impl ThemePalette {
    pub fn new(palette: HashMap<String, Color>) -> Self {
        let ThemePalette {
            palette: mut default,
        } = ThemePalette::default();

        default.extend(palette);
        Self { palette: default }
    }

    pub fn string_to_rgb(s: &str) -> Result<Color, String> {
        if s.starts_with('#') {
            Color::from_hex(s).map_err(|e| format!("{e}: {s}"))
        } else {
            Self::ansi_string_to_rgb(s)
        }
    }

    fn ansi_string_to_rgb(s: &str) -> Result<Color, String> {
        if let Ok(index) = s.parse::<u8>() {
            return Ok(Color::Indexed(index));
        }
        Err(format!("Malformed ANSI: {}", s))
    }

    fn parse_value_as_str(value: &Value) -> Result<&str, String> {
        value
            .as_str()
            .ok_or(format!("Unrecognized value: {}", value))
    }

    pub fn parse_color(&self, value: Value) -> Result<Color, String> {
        let value = Self::parse_value_as_str(&value)?;

        self.palette
            .get(value)
            .copied()
            .ok_or("")
            .or_else(|_| Self::string_to_rgb(value))
    }

    pub fn parse_modifier(value: &Value) -> Result<Modifier, String> {
        value
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or(format!("Invalid modifier: {}", value))
    }

    pub fn parse_underline_style(value: &Value) -> Result<UnderlineStyle, String> {
        value
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or(format!("Invalid underline style: {}", value))
    }

    pub fn parse_style(&self, style: &mut Style, value: Value) -> Result<(), String> {
        if let Value::Table(entries) = value {
            for (name, mut value) in entries {
                match name.as_str() {
                    "fg" => *style = style.fg(self.parse_color(value)?),
                    "bg" => *style = style.bg(self.parse_color(value)?),
                    "underline" => {
                        let table = value.as_table_mut().ok_or("Underline must be table")?;
                        if let Some(value) = table.remove("color") {
                            *style = style.underline_color(self.parse_color(value)?);
                        }
                        if let Some(value) = table.remove("style") {
                            *style = style.underline_style(Self::parse_underline_style(&value)?);
                        }

                        if let Some(attr) = table.keys().next() {
                            return Err(format!("Invalid underline attribute: {attr}"));
                        }
                    }
                    "modifiers" => {
                        let modifiers = value.as_array().ok_or("Modifiers should be an array")?;

                        for modifier in modifiers {
                            if modifier.as_str() == Some("underlined") {
                                *style = style.underline_style(UnderlineStyle::Line);
                            } else {
                                *style = style.add_modifier(Self::parse_modifier(modifier)?);
                            }
                        }
                    }
                    _ => return Err(format!("Invalid style attribute: {}", name)),
                }
            }
        } else {
            *style = style.fg(self.parse_color(value)?);
        }
        Ok(())
    }

    fn parse_style_array(&self, value: Value) -> Result<Vec<Style>, String> {
        let mut styles = Vec::new();

        for v in value
            .as_array()
            .ok_or_else(|| format!("Could not parse value as an array: '{value}'"))?
        {
            let mut style = Style::default();
            self.parse_style(&mut style, v.clone())?;
            styles.push(style);
        }

        Ok(styles)
    }
}

impl TryFrom<Value> for ThemePalette {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let map = match value {
            Value::Table(entries) => entries,
            _ => return Ok(Self::default()),
        };

        let mut palette = HashMap::with_capacity(map.len());
        for (name, value) in map {
            let value = Self::parse_value_as_str(&value)?;
            let color = Self::string_to_rgb(value)?;
            palette.insert(name, color);
        }

        Ok(Self::new(palette))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme(directory: &Path, filename: &str, source: &str) {
        let themes = directory.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join(filename), source).unwrap();
    }

    #[test]
    fn theme_names_include_all_formats_without_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        for filename in [
            "shared.toml",
            "shared.json",
            "shared.jsonc",
            "json.json",
            "jsonc.jsonc",
            "ignored.txt",
        ] {
            write_theme(directory.path(), filename, "");
        }
        std::fs::create_dir(directory.path().join("themes/directory.json")).unwrap();
        assert_eq!(
            Loader::read_names(&directory.path().join("themes")),
            ["json", "jsonc", "shared"]
        );
        assert!(Loader::read_names(&directory.path().join("missing")).is_empty());
    }

    #[test]
    fn theme_formats_produce_equivalent_styles() {
        let directory = tempfile::tempdir().unwrap();
        let loader = Loader::new(&[directory.path().to_owned()]);
        for (filename, source) in [
            (
                "toml.toml",
                r##"
                "ui.text" = "accent"
                "keyword" = { fg = "accent", modifiers = ["bold"], underline = { color = "accent", style = "curl" } }
                rainbow = ["accent", { fg = "#ffffff" }]
                [palette]
                accent = "#123456"
            "##,
            ),
            (
                "json.json",
                r##"{"ui.text":"accent","keyword":{"fg":"accent","modifiers":["bold"],"underline":{"color":"accent","style":"curl"}},"rainbow":["accent",{"fg":"#ffffff"}],"palette":{"accent":"#123456"}}"##,
            ),
            (
                "jsonc.jsonc",
                r##"{
                // Scope names containing dots are literal keys.
                "ui.text": "accent",
                "keyword": {"fg":"accent", "modifiers":["bold",], "underline":{"color":"accent", "style":"curl",},},
                "rainbow": ["accent", {"fg":"#ffffff"},],
                "palette": {/* custom colors */ "accent":"#123456",},
            }"##,
            ),
        ] {
            write_theme(directory.path(), filename, source);
        }
        let (expected, warnings) = loader.load_with_warnings("toml").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        for name in ["json", "jsonc"] {
            let (actual, warnings) = loader.load_with_warnings(name).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(actual.styles, expected.styles);
            assert_eq!(actual.rainbow_length(), expected.rainbow_length());
        }
        assert_eq!(
            expected.get("ui.text").fg,
            Some(Color::Rgb(0x12, 0x34, 0x56))
        );
    }

    #[test]
    fn mixed_format_inheritance_preserves_palette_merging_and_style_replacement() {
        let parents = [
            (
                "toml",
                r##"
                "ui.text" = "accent"
                "ui.background" = { bg = "background" }
                keyword = { fg = "accent", bg = "background", modifiers = ["bold"] }
                [palette]
                accent = "#112233"
                background = "#223344"
            "##,
            ),
            (
                "json",
                r##"{"ui.text":"accent","ui.background":{"bg":"background"},"keyword":{"fg":"accent","bg":"background","modifiers":["bold"]},"palette":{"accent":"#112233","background":"#223344"}}"##,
            ),
            (
                "jsonc",
                r##"{"ui.text":"accent","ui.background":{"bg":"background"},"keyword":{"fg":"accent","bg":"background","modifiers":["bold",],},"palette":{"accent":"#112233","background":"#223344",},}"##,
            ),
        ];
        let children = [
            (
                "toml",
                "inherits = 'parent'\nkeyword = { fg = 'accent' }\n[palette]\naccent = '#445566'",
            ),
            (
                "json",
                r##"{"inherits":"parent","keyword":{"fg":"accent"},"palette":{"accent":"#445566"}}"##,
            ),
            (
                "jsonc",
                r##"{"inherits":"parent",/* override */"keyword":{"fg":"accent",},"palette":{"accent":"#445566",},}"##,
            ),
        ];
        for (parent_extension, parent) in parents {
            for (child_extension, child) in children {
                let directory = tempfile::tempdir().unwrap();
                write_theme(
                    directory.path(),
                    &format!("parent.{parent_extension}"),
                    parent,
                );
                write_theme(directory.path(), &format!("child.{child_extension}"), child);
                // A third level confirms inherited palette changes propagate through a chain.
                write_theme(
                    directory.path(),
                    "grandchild.jsonc",
                    r##"{"inherits":"child","palette":{"background":"#778899",},}"##,
                );
                let loader = Loader::new(&[directory.path().to_owned()]);
                let (theme, warnings) = loader.load_with_warnings("grandchild").unwrap();
                assert!(
                    warnings.is_empty(),
                    "{parent_extension} -> {child_extension}: {warnings:?}"
                );
                let accent = Style::default().fg(Color::Rgb(0x44, 0x55, 0x66));
                assert_eq!(theme.get("ui.text"), accent);
                // Child style tables replace rather than merge parent style fields.
                assert_eq!(theme.get("keyword"), accent);
                assert_eq!(
                    theme.get("ui.background"),
                    Style::default().bg(Color::Rgb(0x77, 0x88, 0x99))
                );
            }
        }
    }

    #[test]
    fn theme_priority_and_same_name_inheritance_work_across_formats() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user");
        let runtime = directory.path().join("runtime");
        write_theme(
            &runtime,
            "shared.toml",
            "\"ui.text\" = '#112233'\n\"ui.background\" = { bg = '#223344' }",
        );
        let loader = Loader::new(&[user.clone(), runtime]);
        for (filename, source, expected) in [
            (
                "shared.jsonc",
                r##"{"inherits":"shared","ui.text":"#334455",}"##,
                Color::Rgb(0x33, 0x44, 0x55),
            ),
            (
                "shared.json",
                r##"{"inherits":"shared","ui.text":"#445566"}"##,
                Color::Rgb(0x44, 0x55, 0x66),
            ),
            (
                "shared.toml",
                "inherits = 'shared'\n\"ui.text\" = '#556677'",
                Color::Rgb(0x55, 0x66, 0x77),
            ),
        ] {
            write_theme(&user, filename, source);
            let (theme, warnings) = loader.load_with_warnings("shared").unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(theme.get("ui.text").fg, Some(expected));
            assert_eq!(
                theme.get("ui.background").bg,
                Some(Color::Rgb(0x22, 0x33, 0x44))
            );
        }
        // Invalid higher-priority files must not silently load a different theme.
        write_theme(&user, "shared.toml", "invalid = [");
        assert!(loader.load("shared").is_err());
    }

    #[test]
    fn json_themes_can_inherit_reserved_builtin_themes() {
        let directory = tempfile::tempdir().unwrap();
        let loader = Loader::new(&[directory.path().to_owned()]);
        for (name, source) in [
            ("default", DEFAULT_THEME_DATA.clone()),
            ("base16_default", BASE16_DEFAULT_THEME_DATA.clone()),
        ] {
            // User files still cannot replace the built-in themes.
            write_theme(directory.path(), &format!("{name}.json"), "invalid JSON");
            assert_eq!(
                loader.load(name).unwrap().styles,
                Theme::from(source).styles
            );
            write_theme(
                directory.path(),
                "child.jsonc",
                &format!(r##"{{"inherits":"{name}","ui.text":"#abcdef",}}"##),
            );
            let (theme, warnings) = loader.load_with_warnings("child").unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(theme.get("ui.text").fg, Some(Color::Rgb(0xab, 0xcd, 0xef)));
            assert_eq!(
                theme.get("ui.background"),
                loader.load(name).unwrap().get("ui.background")
            );
        }
    }

    #[test]
    fn theme_loader_reports_invalid_json_and_inheritance_errors() {
        let directory = tempfile::tempdir().unwrap();
        let loader = Loader::new(&[directory.path().to_owned()]);
        for (filename, source) in [
            ("bad.json", r##"{"ui.text":"#ffffff",}"##),
            ("bad.jsonc", "{} /* unterminated"),
        ] {
            write_theme(directory.path(), filename, source);
            let error = loader.load("bad").unwrap_err().to_string();
            assert!(error.contains(filename), "{error}");
            std::fs::remove_file(directory.path().join("themes").join(filename)).unwrap();
        }
        write_theme(directory.path(), "a.json", r#"{"inherits":"b"}"#);
        write_theme(directory.path(), "b.jsonc", r#"{"inherits":"a",}"#);
        assert!(loader
            .load("a")
            .unwrap_err()
            .to_string()
            .contains("Cycle found"));
        write_theme(directory.path(), "b.jsonc", r#"{"inherits":"missing"}"#);
        assert!(loader
            .load("a")
            .unwrap_err()
            .to_string()
            .contains("File not found for: missing"));
        write_theme(directory.path(), "b.jsonc", r#"{"inherits":1}"#);
        assert!(loader
            .load("a")
            .unwrap_err()
            .to_string()
            .contains("Expected 'inherits' to be a string"));
    }

    #[test]
    fn test_parse_style_string() {
        let fg = Value::String("#ffffff".to_string());

        let mut style = Style::default();
        let palette = ThemePalette::default();
        palette.parse_style(&mut style, fg).unwrap();

        assert_eq!(style, Style::default().fg(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn test_palette() {
        use helix_core::hashmap;
        let fg = Value::String("my_color".to_string());

        let mut style = Style::default();
        let palette =
            ThemePalette::new(hashmap! { "my_color".to_string() => Color::Rgb(255, 255, 255) });
        palette.parse_style(&mut style, fg).unwrap();

        assert_eq!(style, Style::default().fg(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn test_parse_style_table() {
        let table = toml::toml! {
            "keyword" = {
                fg = "#ffffff",
                bg = "#000000",
                modifiers = ["bold"],
            }
        };

        let mut style = Style::default();
        let palette = ThemePalette::default();
        for (_name, value) in table {
            palette.parse_style(&mut style, value).unwrap();
        }

        assert_eq!(
            style,
            Style::default()
                .fg(Color::Rgb(255, 255, 255))
                .bg(Color::Rgb(0, 0, 0))
                .add_modifier(Modifier::BOLD)
        );
    }

    // tests for parsing an RGB `Highlight`

    #[test]
    fn convert_to_and_from() {
        let (r, g, b) = (0xFF, 0xFE, 0xFA);
        let highlight = Theme::rgb_highlight(r, g, b);
        assert_eq!(Theme::decode_rgb_highlight(highlight), Some((r, g, b)));
    }

    /// make sure we can store all the colors at the end
    #[test]
    fn full_numeric_range() {
        assert_eq!(Highlight::MAX - Theme::RGB_START, 256_u32.pow(3));
    }

    #[test]
    fn retrieve_color() {
        // color in the middle
        let (r, g, b) = (0x14, 0xAA, 0xF7);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
        // pure black
        let (r, g, b) = (0x00, 0x00, 0x00);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
        // pure white
        let (r, g, b) = (0xff, 0xff, 0xff);
        assert_eq!(
            Theme::default().highlight(Theme::rgb_highlight(r, g, b)),
            Style::new().fg(Color::Rgb(r, g, b))
        );
    }

    #[test]
    #[should_panic(expected = "index out of bounds: the len is 0 but the index is 4278190078")]
    fn out_of_bounds() {
        let highlight = Highlight::new(Theme::rgb_highlight(0, 0, 0).get() - 1);
        Theme::default().highlight(highlight);
    }
}
