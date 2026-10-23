//! LSP-specific completion presentation.
//! Only display text is reconstructed; completion edits and filter text are untouched.

// The MIT Licence
//
// Copyright (c) 2021 Lukas Reineke
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use std::ops::Range;

use helix_lsp::lsp::{CompletionItem, CompletionItemKind as K, InsertTextFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Server {
    Rust,
    Go,
    Lua,
    Clang,
    TypeScript,
    Vtsls,
    Zig,
    Php,
    Roslyn,
    Python,
    Pylsp,
    Dart,
    Other,
}

impl Server {
    pub fn from_name(name: &str) -> Self {
        match name {
            "rust-analyzer" | "rust_analyzer" => Self::Rust,
            "gopls" => Self::Go,
            "lua_ls" | "lua-ls" | "lua-language-server" => Self::Lua,
            "clangd" => Self::Clang,
            "typescript-language-server" | "ts_ls" | "tsserver" | "typescript-tools" => {
                Self::TypeScript
            }
            "vtsls" => Self::Vtsls,
            "zls" => Self::Zig,
            "intelephense" => Self::Php,
            "roslyn" | "roslyn_ls" | "roslyn-ls" | "roslyn-language-server" | "easy_dotnet" => {
                Self::Roslyn
            }
            "basedpyright" | "pyright" | "pylance" => Self::Python,
            "pylsp" | "python-lsp-server" => Self::Pylsp,
            "dartls" | "dart" | "dart-language-server" => Self::Dart,
            _ => Self::Other,
        }
    }

    pub fn language(self, current: Option<&str>) -> Option<&str> {
        Some(match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Lua => "lua",
            Self::Clang => {
                if current == Some("c") {
                    "c"
                } else {
                    "cpp"
                }
            }
            Self::TypeScript | Self::Vtsls => "typescript",
            Self::Zig => "zig",
            Self::Php => "php",
            Self::Roslyn => "c-sharp",
            Self::Python | Self::Pylsp => "python",
            Self::Dart => "dart",
            Self::Other => return None,
        })
    }
}

/// A range of display text projected out of a valid synthetic source file.
pub(super) struct Projection {
    pub source: Range<usize>,
    pub target: Range<usize>,
}

pub(super) struct Label {
    pub left: String,
    pub right: String,
    pub extra: String,
    pub source: String,
    pub projections: Vec<Projection>,
    pub dim_arguments: bool,
    pub right_is_metadata: bool,
    pub left_scope: Option<&'static str>,
}

pub(super) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Find a balanced parameter list, including nested function types and strings.
fn parameters(text: &str) -> Option<Range<usize>> {
    let start = text.find('(')?;
    let rust_lifetimes = text.contains("fn ");
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote.is_some() && ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '"' | '\'') {
            let tail = &text[start + offset + ch.len_utf8()..];
            let character_literal = tail.starts_with('\\')
                || tail
                    .chars()
                    .next()
                    .is_some_and(|first| tail[first.len_utf8()..].starts_with('\''));
            if ch == '"' || !rust_lifetimes || character_literal {
                quote = Some(ch);
            }
        }
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start..start + offset + 1);
                }
            }
            _ => (),
        }
    }
    None
}

fn set_signature(label: &mut Label, signature: &str, return_separator: &str) {
    if let Some(params) = parameters(signature) {
        let chain = if label.left.starts_with("iter().") {
            "iter().".len()
        } else {
            0
        };
        let name_end = label.left[chain..]
            .find('(')
            .map_or(label.left.len(), |index| chain + index);
        label.left.truncate(name_end);
        label.left.push_str(&signature[params.clone()]);
        let ret = signature[params.end..].trim();
        label.right = ret
            .strip_prefix(return_separator)
            .unwrap_or(ret)
            .trim()
            .trim_end_matches(';')
            .into();
    }
}

impl Label {
    fn new(item: &CompletionItem) -> Self {
        Self {
            left: one_line(&item.label),
            right: String::new(),
            extra: String::new(),
            source: String::new(),
            projections: Vec::new(),
            dim_arguments: false,
            right_is_metadata: false,
            left_scope: None,
        }
    }

    /// Append display text to source, remembering where its colors belong.
    fn project(&mut self, text: &str, target_start: usize) {
        let source_start = self.source.len();
        self.source.push_str(text);
        self.projections.push(Projection {
            source: source_start..self.source.len(),
            target: target_start..target_start + text.len(),
        });
    }

    fn source_for(&mut self, server: Server, kind: Option<K>) {
        let left = self.left.clone();
        let right = self.right.clone();
        let right_start = left.len() + 2;
        let callable = matches!(kind, Some(K::FUNCTION | K::METHOD | K::CONSTRUCTOR));
        if self.right_is_metadata {
            // Metadata is intentionally separate from the parseable signature.
            return;
        }
        match server {
            Server::Rust => {
                if self.left_scope == Some("constructor") {
                    self.source.push_str("match value { ");
                    self.project(&left, 0);
                    self.source.push_str(" => {} }");
                    return;
                }
                if kind == Some(K::ENUM_MEMBER) {
                    self.source.push_str("enum S { ");
                    self.project(&left, 0);
                    self.source.push_str(" }");
                    return;
                }
                self.source
                    .push_str(if callable { "fn " } else { "struct S { " });
                let prefix = if callable {
                    if left.starts_with("iter().") {
                        "iter().".len()
                    } else if left.starts_with("self.") {
                        "self.".len()
                    } else {
                        0
                    }
                } else {
                    0
                };
                self.project(&left[prefix..], prefix);
                if !right.is_empty() {
                    self.source.push_str(if callable { " -> " } else { ": " });
                    self.project(&right, right_start);
                }
                self.source.push_str(if callable { " {}" } else { " }" });
            }
            Server::Go => {
                self.source.push_str(if callable {
                    "package p\nfunc "
                } else {
                    "package p\nvar "
                });
                // Gopls can return chained selectors: color the final declaration
                // in syntactically valid context without dropping the prefix.
                let prefix = left
                    .split('(')
                    .next()
                    .unwrap_or(&left)
                    .rfind('.')
                    .map_or(0, |p| p + 1);
                self.project(&left[prefix..], prefix);
                if !right.is_empty() {
                    self.source.push(' ');
                    self.project(&right, right_start);
                }
                if callable {
                    self.source.push_str(" {} ");
                }
            }
            Server::Clang => {
                if !callable {
                    self.source.push_str("void f(){ ");
                }
                if right.is_empty() {
                    self.source.push_str("void ");
                } else {
                    self.project(&right, right_start);
                    self.source.push(' ');
                }
                // clangd's leading dot denotes a not-yet-imported completion.
                let prefix = if left.starts_with('•') {
                    '•'.len_utf8()
                } else {
                    0
                };
                self.project(&left[prefix..], prefix);
                self.source.push(';');
                if !callable {
                    self.source.push('}');
                }
            }
            Server::Zig => {
                self.source.push_str(if callable { "fn " } else { "fn f(" });
                self.project(&left, 0);
                if !right.is_empty() {
                    self.source.push_str(if callable { " " } else { ": " });
                    self.project(&right, right_start);
                }
                self.source
                    .push_str(if callable { " {}" } else { ") void {}" });
            }
            Server::Php => {
                self.source.push_str(if callable {
                    "<?php function "
                } else {
                    "<?php function f(): "
                });
                if callable {
                    self.project(&left, 0);
                }
                if !right.is_empty() {
                    if callable {
                        self.source.push_str(": ");
                    }
                    self.project(&right, right_start);
                }
                self.source.push_str(" {}");
            }
            Server::Dart => {
                if !right.is_empty() {
                    self.project(&right, right_start);
                    self.source.push(' ');
                } else {
                    self.source.push_str("void ");
                }
                self.project(&left, 0);
                self.source.push_str(if callable { " {}" } else { ";" });
            }
            Server::Lua => {
                self.source.push_str("v.");
                self.project(&left, 0);
            }
            _ => {
                self.project(&left, 0);
            }
        }
    }
}

pub(super) fn format(item: &CompletionItem, server: Server) -> Label {
    let mut label = Label::new(item);
    let details = item.label_details.as_ref();
    let short = details
        .and_then(|d| d.detail.as_deref())
        .filter(|s| !s.is_empty());
    let description = details
        .and_then(|d| d.description.as_deref())
        .filter(|s| !s.is_empty());
    let detail = short
        .or(item.detail.as_deref())
        .map(one_line)
        .unwrap_or_default();
    let full = item.detail.as_deref().map(one_line).unwrap_or_default();
    let callable = matches!(item.kind, Some(K::FUNCTION | K::METHOD | K::CONSTRUCTOR));
    match server {
        Server::Rust => {
            let variant_snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET)
                && label.left.chars().next().is_some_and(char::is_uppercase)
                && (label.left.contains("::")
                    || ["Some(…)", "None", "Ok(…)", "Err(…)"].contains(&label.left.as_str()));
            if variant_snippet {
                label.left_scope = Some("constructor");
            } else if callable {
                let signature = description.filter(|s| s.contains("fn ")).unwrap_or(&full);
                if signature.contains("fn ") {
                    set_signature(&mut label, signature, "->");
                } else if let Some(short) = short {
                    set_signature(&mut label, short, "->");
                }
                if let Some(start) = item
                    .label
                    .find("(use ")
                    .or_else(|| item.label.find("(as "))
                    .or_else(|| item.label.find("(alias "))
                {
                    label.extra = one_line(&item.label[start..]);
                }
            } else if item.kind == Some(K::ENUM_MEMBER) && !detail.is_empty() {
                label.left = detail.clone();
            } else if matches!(item.kind, Some(K::FIELD | K::VARIABLE | K::CONSTANT))
                && (item.kind == Some(K::FIELD)
                    || item.insert_text_format != Some(InsertTextFormat::SNIPPET))
            {
                label.right =
                    if matches!(item.kind, Some(K::VARIABLE | K::CONSTANT)) && !full.is_empty() {
                        full
                    } else {
                        detail
                    };
            } else if detail.starts_with('(') {
                label.right = detail;
                label.right_is_metadata = true;
            }
        }
        Server::Go => {
            if let Some((_, path)) = detail.rsplit_once("(from ") {
                label.right = path.trim_end_matches(')').trim().into();
                label.right_is_metadata = true;
            } else if callable {
                set_signature(
                    &mut label,
                    detail.strip_prefix("func").unwrap_or(&detail).trim(),
                    "",
                );
            } else {
                label.right = match item.kind {
                    Some(K::STRUCT) => "struct{}".into(),
                    Some(K::INTERFACE) => "interface{}".into(),
                    _ => detail,
                };
            }
        }
        Server::Clang => {
            if let Some(signature) = short.filter(|_| callable && !label.left.contains('(')) {
                label.left.push_str(&one_line(signature));
            }
            if matches!(
                item.kind,
                Some(K::FUNCTION | K::METHOD | K::VARIABLE | K::CONSTANT | K::FIELD)
            ) {
                label.right = full;
            }
            if let Some(lsp_documentation) = &item.documentation {
                let text = match lsp_documentation {
                    helix_lsp::lsp::Documentation::String(text) => text,
                    helix_lsp::lsp::Documentation::MarkupContent(text) => &text.value,
                };
                if let Some(path) = text
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("From "))
                {
                    label.extra = path.trim().trim_matches('`').into();
                }
            }
        }
        Server::Lua => {
            label.dim_arguments = callable;
        }
        Server::TypeScript | Server::Vtsls | Server::Roslyn | Server::Python | Server::Pylsp => {
            // These servers expose origin/description metadata rather than a
            // reconstructable signature. Do not mistake it for a return type.
            label.right = match server {
                Server::TypeScript => full,
                Server::Vtsls => description.map(one_line).unwrap_or(full),
                _ => description.map(one_line).unwrap_or_default(),
            };
            if matches!(server, Server::TypeScript | Server::Vtsls) && label.right.is_empty() {
                label.right = detail;
            }
            label.right_is_metadata = true;
            label.dim_arguments = server == Server::Pylsp && callable;
        }
        Server::Zig => {
            let zig_detail = item
                .detail
                .as_deref()
                .or(short)
                .unwrap_or_default()
                .lines()
                .next()
                .unwrap_or_default();
            if callable && zig_detail.starts_with("fn") {
                if let (Some(params), Some(ret)) = (short, description) {
                    set_signature(&mut label, &format!("{params} {}", one_line(ret)), "");
                } else {
                    set_signature(&mut label, zig_detail, "");
                }
            } else if callable && zig_detail.starts_with('@') {
                label.left = one_line(zig_detail);
            } else if zig_detail == "type" {
                label.left_scope = Some("type");
            } else {
                label.right = one_line(zig_detail);
            }
        }
        Server::Php => {
            if callable {
                set_signature(&mut label, &detail, ":");
            } else {
                label.right = detail;
            }
        }
        Server::Dart => {
            let clean = item.detail.as_deref().unwrap_or(&detail);
            let clean = clean
                .rsplit_once("\n\n")
                .map_or(clean, |(_, rest)| rest)
                .lines()
                .next()
                .unwrap_or_default();
            if callable {
                set_signature(&mut label, clean, "→");
                if item.kind == Some(K::CONSTRUCTOR) {
                    label.right.clear();
                }
            } else {
                label.right = one_line(clean);
            }
        }
        Server::Other => {
            label.right = if detail.is_empty() {
                description.map(one_line).unwrap_or_default()
            } else {
                detail
            };
            label.right_is_metadata = true;
        }
    }
    label.source_for(server, item.kind);
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_each_supported_servers_display_fields() {
        for (server, item, left, right) in [
            (
                "rust-analyzer",
                serde_json::json!({"label":"find(…)","kind":3,"detail":"pub fn find(value: i32) -> Option<String>"}),
                "find(value: i32)",
                "Option<String>",
            ),
            (
                "gopls",
                serde_json::json!({"label":"strings.Join","kind":3,"detail":"func(a []string, sep string) string"}),
                "strings.Join(a []string, sep string)",
                "string",
            ),
            (
                "clangd",
                serde_json::json!({"label":"print","kind":3,"detail":"int","labelDetails":{"detail":"(const char *text)"}}),
                "print(const char *text)",
                "int",
            ),
            (
                "lua-language-server",
                serde_json::json!({"label":"print(value)","kind":3}),
                "print(value)",
                "",
            ),
            (
                "typescript-language-server",
                serde_json::json!({"label":"value","kind":6,"detail":"(property) value: string"}),
                "value",
                "(property) value: string",
            ),
            (
                "vtsls",
                serde_json::json!({"label":"value","kind":6,"detail":"ignored","labelDetails":{"description":"source.ts"}}),
                "value",
                "source.ts",
            ),
            (
                "zls",
                serde_json::json!({"label":"read","kind":3,"detail":"fn(buf: []u8) usize","labelDetails":{"detail":"(buf: []u8)","description":"usize"}}),
                "read(buf: []u8)",
                "usize",
            ),
            (
                "intelephense",
                serde_json::json!({"label":"fetch","kind":3,"detail":"fetch(string $url): Response"}),
                "fetch(string $url)",
                "Response",
            ),
            (
                "roslyn",
                serde_json::json!({"label":"GetValue","kind":2,"labelDetails":{"description":"System.Int32"}}),
                "GetValue",
                "System.Int32",
            ),
            (
                "basedpyright",
                serde_json::json!({"label":"Path","kind":7,"labelDetails":{"description":"pathlib"}}),
                "Path",
                "pathlib",
            ),
            (
                "pylsp",
                serde_json::json!({"label":"open(file)","kind":3,"labelDetails":{"description":"builtins"}}),
                "open(file)",
                "builtins",
            ),
            (
                "dartls",
                serde_json::json!({"label":"fetch(…)","kind":3,"detail":"Auto import from package\n\n(int count) → Future<String>"}),
                "fetch(int count)",
                "Future<String>",
            ),
            (
                "rust-analyzer",
                serde_json::json!({"label":"iter().map(…)","kind":2,"detail":"fn map(f: F) -> Map<Self, F>"}),
                "iter().map(f: F)",
                "Map<Self, F>",
            ),
            (
                "zls",
                serde_json::json!({"label":"Reader","kind":6,"detail":"type"}),
                "Reader",
                "",
            ),
        ] {
            let item: CompletionItem = serde_json::from_value(item).unwrap();
            let result = format(&item, Server::from_name(server));
            assert_eq!(result.left, left, "{server}");
            assert_eq!(result.right, right, "{server}");
            for projection in result.projections {
                assert!(result.source.is_char_boundary(projection.source.start));
                assert!(result.source.is_char_boundary(projection.source.end));
            }
        }
        for alias in [
            "rust_analyzer",
            "lua_ls",
            "ts_ls",
            "tsserver",
            "typescript-tools",
            "roslyn_ls",
            "roslyn-language-server",
            "easy_dotnet",
            "pyright",
            "pylance",
            "python-lsp-server",
            "dart",
        ] {
            assert_ne!(Server::from_name(alias), Server::Other, "{alias}");
        }
    }

    #[test]
    fn handles_nested_signatures_and_import_metadata() {
        let item: CompletionItem = serde_json::from_value(serde_json::json!({"label":"Map","kind":3,"detail":"func(fn func(int) string, values []int) []string"})).unwrap();
        let result = format(&item, Server::Go);
        assert_eq!(result.left, "Map(fn func(int) string, values []int)");
        assert_eq!(result.right, "[]string");
        let item: CompletionItem = serde_json::from_value(serde_json::json!({"label":"mix(…)","kind":3,"detail":"fn mix(a: &'a str, f: fn(&'b str) -> usize) -> &'a str"})).unwrap();
        let result = format(&item, Server::Rust);
        assert_eq!(result.left, "mix(a: &'a str, f: fn(&'b str) -> usize)");
        assert_eq!(result.right, "&'a str");
        let item: CompletionItem = serde_json::from_value(serde_json::json!({"label":"•print","kind":3,"detail":"void","labelDetails":{"detail":"()"},"documentation":{"kind":"markdown","value":"From `<cstdio>`\nMore docs"}})).unwrap();
        assert_eq!(format(&item, Server::Clang).extra, "<cstdio>");
        let item: CompletionItem = serde_json::from_value(serde_json::json!({"label":"Analyzer","kind":6,"detail":"var (from \"example.org/analyzer\")"})).unwrap();
        let label = format(&item, Server::Go);
        assert!(label.right_is_metadata);
        assert_eq!(label.right, "\"example.org/analyzer\"");
    }
}
