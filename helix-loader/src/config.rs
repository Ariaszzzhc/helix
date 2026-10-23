use std::{
    path::{Path, PathBuf},
    str::from_utf8,
};

use serde::de::{DeserializeOwned, Error as _};

use crate::workspace_trust::{TrustQuery, WorkspaceTrust};

/// Default built-in languages.toml.
pub fn default_lang_config() -> toml::Value {
    let default_config = include_bytes!("../../languages.toml");
    toml::from_str(from_utf8(default_config).unwrap())
        .expect("Could not parse built-in languages.toml to valid toml")
}

/// User language configuration (TOML, JSON or JSONC), merged with defaults.
///
/// Workspace-local language configuration is merged in only when the current
/// workspace is trusted for [`TrustQuery::LocalConfig`].
pub fn user_lang_config(trust: &WorkspaceTrust) -> Result<toml::Value, toml::de::Error> {
    let global_config = crate::lang_config_file();
    let workspace_config = crate::workspace_lang_config_file();

    let files = if trust.query_current(TrustQuery::LocalConfig).is_trusted() {
        vec![global_config, workspace_config]
    } else {
        vec![global_config]
    };

    let config = files
        .iter()
        .filter_map(|file| {
            std::fs::read_to_string(file)
                .map(|config| parse(&config, file))
                .ok()
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(default_lang_config(), |a, b| {
            crate::merge_toml_values(a, b, 3)
        });

    Ok(config)
}

/// Select one configuration per directory. Preserve TOML's priority and use it
/// as the creation path when no configuration exists yet.
pub fn find_config_file(directory: &Path, stem: &str) -> PathBuf {
    ["toml", "json", "jsonc"]
        .into_iter()
        .map(|extension| directory.join(format!("{stem}.{extension}")))
        .find(|path| path.exists())
        .unwrap_or_else(|| directory.join(format!("{stem}.toml")))
}

/// Parse a configuration using its filename extension. Extensionless and custom
/// extensions retain the historical TOML behavior of `hx --config`.
///
/// JSON values are deserialized into the same table representation as TOML so
/// validation and merging are identical. Null has no configuration equivalent.
pub fn parse<T: DeserializeOwned>(source: &str, path: &Path) -> Result<T, toml::de::Error> {
    let extension = path.extension().and_then(|extension| extension.to_str());
    let result = match extension {
        Some("json" | "jsonc") => {
            let jsonc;
            let source = if extension == Some("jsonc") {
                jsonc = normalize_jsonc(source)
                    .map_err(|err| toml::de::Error::custom(format!("{}: {err}", path.display())))?;
                &jsonc
            } else {
                source
            };
            serde_json::from_str::<toml::Table>(source)
                .map_err(toml::de::Error::custom)
                .and_then(|table| toml::Value::Table(table).try_into())
        }
        _ => toml::from_str(source),
    };
    result.map_err(|err| toml::de::Error::custom(format!("{}: {err}", path.display())))
}

/// Replace comments and trailing commas with spaces, preserving byte offsets
/// and newlines for serde_json's error locations. String contents stay intact.
fn normalize_jsonc(source: &str) -> Result<String, &'static str> {
    let mut bytes = source.as_bytes().to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                    bytes[i] = b' ';
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                bytes[i..i + 2].fill(b' ');
                i += 2;
                loop {
                    if i >= bytes.len() {
                        return Err("unterminated JSONC block comment");
                    }
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        bytes[i..i + 2].fill(b' ');
                        i += 2;
                        break;
                    }
                    if !matches!(bytes[i], b'\r' | b'\n') {
                        bytes[i] = b' ';
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    i = 0;
    let mut previous = None;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                previous = Some(b'"');
                continue;
            }
            b',' if !matches!(previous, None | Some(b'[' | b'{' | b',' | b':')) => {
                let next = bytes[i + 1..]
                    .iter()
                    .find(|byte| !byte.is_ascii_whitespace());
                if matches!(next, Some(b']' | b'}')) {
                    bytes[i] = b' ';
                }
                previous = Some(b',');
            }
            byte if !byte.is_ascii_whitespace() => previous = Some(byte),
            _ => {}
        }
        i += 1;
    }
    // Only ASCII punctuation and comment bytes were replaced. Entire comments,
    // including all bytes of any UTF-8 characters, have been replaced by spaces.
    Ok(String::from_utf8(bytes).expect("JSONC normalization preserves UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_discovery_preserves_toml_priority() {
        let dir = tempfile::tempdir().unwrap();
        for stem in ["config", "languages"] {
            assert_eq!(
                find_config_file(dir.path(), stem),
                dir.path().join(format!("{stem}.toml"))
            );
            for extension in ["jsonc", "json", "toml"] {
                let path = dir.path().join(format!("{stem}.{extension}"));
                std::fs::write(&path, "").unwrap();
                assert_eq!(find_config_file(dir.path(), stem), path);
            }
        }
    }

    #[test]
    fn formats_produce_the_same_configuration() {
        let toml: toml::Value =
            parse("[editor]\nmouse = false\n", Path::new("config.toml")).unwrap();
        for (extension, source) in [
            ("json", r#"{"editor":{"mouse":false}}"#),
            (
                "jsonc",
                "{/* 注释 */\n\"editor\": {\"mouse\": false,}, // comment\n}",
            ),
        ] {
            let value: toml::Value =
                parse(source, Path::new(&format!("config.{extension}"))).unwrap();
            assert_eq!(value, toml);
        }
        let custom: toml::Value =
            parse("[editor]\nmouse = false", Path::new("custom.conf")).unwrap();
        assert_eq!(custom, toml);
    }

    #[test]
    fn jsonc_preserves_strings_and_error_positions() {
        let source = r#"{"url":"https://example.com/a/*b*/", "text":"escaped \" // ,}",}"#;
        let value: toml::Value = parse(source, Path::new("config.jsonc")).unwrap();
        assert_eq!(value["url"].as_str(), Some("https://example.com/a/*b*/"));
        assert_eq!(value["text"].as_str(), Some("escaped \" // ,}"));
        let source = "{/* 注释 */\n\"a\": ?}";
        let error = parse::<toml::Value>(source, Path::new("config.jsonc"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("config.jsonc"), "{error}");
        assert!(error.contains("line 2 column 6"), "{error}");
    }

    #[test]
    fn invalid_json_and_unsupported_values_are_rejected() {
        for source in [
            r#"{"a":null}"#,
            "[]",
            "null",
            r#"{"a":1,}"#,
            "{/*comment*/}",
        ] {
            assert!(
                parse::<toml::Value>(source, Path::new("config.json")).is_err(),
                "{source}"
            );
        }
        for source in [
            "{,}",
            r#"{"a":[,]}"#,
            r#"{"a":[1,,]}"#,
            r#"{"a":,}"#,
            "{} /* unterminated",
            "{'a': 1}",
            "{a: 1}",
        ] {
            assert!(
                parse::<toml::Value>(source, Path::new("config.jsonc")).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn language_overrides_merge_across_formats() {
        let global: toml::Value = parse(
            "[[language]]\nname = 'rust'\nauto-format = true\n",
            Path::new("languages.toml"),
        )
        .unwrap();
        let local = parse::<toml::Value>(
            r#"{"language": [{"name": "rust", "tab-width": 4,}],}"#,
            Path::new("languages.jsonc"),
        )
        .unwrap();
        let merged = crate::merge_toml_values(global, local, 3);
        let rust = &merged["language"][0];
        assert_eq!(rust["auto-format"].as_bool(), Some(true));
        assert_eq!(rust["tab-width"].as_integer(), Some(4));
    }
}
