use crate::ui::kind::{scope_for_name, scope_style};
use std::{collections::HashMap, ops::Range, sync::Arc};

use arc_swap::ArcSwap;
use helix_core::{
    fuzzy::MATCHER,
    syntax::{self, HighlightEvent},
    unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr},
    RopeSlice, Syntax,
};
use helix_lsp::{lsp, LanguageServerId};
use helix_view::{
    graphics::{Modifier, Style},
    Theme,
};
use nucleo::{
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
    Utf32Str,
};
use parking_lot::{Mutex, RwLock};
use tui::text::{Span, Spans};

use super::format::{self, Server};

/// Per-menu cache. No syntax parsing is performed on repeated paints or while
/// the user filters an existing response. Resolved items have a different key.
pub(super) struct Highlighter {
    loader: Arc<ArcSwap<syntax::Loader>>,
    language: Option<String>,
    servers: HashMap<LanguageServerId, String>,
    cache: Mutex<HashMap<Key, Prepared>>,
    pub filter: Arc<RwLock<String>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    server: String,
    label: String,
    detail: Option<String>,
    short: Option<String>,
    description: Option<String>,
    origin: Option<String>,
    kind: Option<String>,
    snippet: bool,
}

#[derive(Clone)]
struct Prepared {
    spans: Spans<'static>,
    left_bytes: usize,
}

impl Highlighter {
    pub fn new(editor: &helix_view::Editor) -> Self {
        Self {
            loader: editor.syn_loader.clone(),
            language: helix_view::doc!(editor).language_name().map(str::to_owned),
            servers: editor
                .language_servers
                .iter_clients()
                .map(|ls| (ls.id(), ls.name().to_owned()))
                .collect(),
            cache: Mutex::new(HashMap::new()),
            filter: Arc::default(),
        }
    }

    pub fn label(
        &self,
        item: &super::CompletionItem,
        theme: &Theme,
        width: usize,
    ) -> Spans<'static> {
        let prepared = match item {
            super::CompletionItem::Lsp(completion) => {
                let item = &completion.item;
                let server = self
                    .servers
                    .get(&completion.provider)
                    .map(String::as_str)
                    .unwrap_or_default();
                let origin = match item.documentation.as_ref() {
                    Some(lsp::Documentation::String(text)) => Some(text.as_str()),
                    Some(lsp::Documentation::MarkupContent(text)) => Some(text.value.as_str()),
                    None => None,
                }
                .and_then(|text| text.lines().next())
                .filter(|line| line.starts_with("From "))
                .map(str::to_owned);
                let key = Key {
                    server: server.to_owned(),
                    label: item.label.clone(),
                    detail: item.detail.clone(),
                    short: item
                        .label_details
                        .as_ref()
                        .and_then(|details| details.detail.clone()),
                    description: item
                        .label_details
                        .as_ref()
                        .and_then(|details| details.description.clone()),
                    kind: item.kind.map(|kind| format!("{kind:?}")),
                    snippet: item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET),
                    origin,
                };
                let mut cache = self.cache.lock();
                if let Some(prepared) = cache.get(&key) {
                    prepared.clone()
                } else {
                    let server = Server::from_name(server);
                    let label = format::format(item, server);
                    let prepared = prepare(
                        &label,
                        item.kind,
                        server.language(self.language.as_deref()),
                        &self.loader.load(),
                        theme,
                        width,
                    );
                    // Bound the lifetime cache even if an incomplete server keeps appending.
                    if cache.len() >= 4096 {
                        cache.clear();
                    }
                    cache.insert(key, prepared.clone());
                    prepared
                }
            }
            super::CompletionItem::Other(item) => {
                let text = format::one_line(&item.label);
                let spans = truncate(
                    Spans::from(Span::styled(
                        text,
                        scope_style(theme, scope_for_name(&item.kind)),
                    )),
                    width,
                );
                let left_bytes = text_len(&spans);
                Prepared { spans, left_bytes }
            }
        };
        match_highlights(prepared, &self.filter.read(), theme)
    }
}

fn kind_scope(kind: Option<lsp::CompletionItemKind>) -> &'static str {
    use lsp::CompletionItemKind as K;
    match kind {
        Some(K::FUNCTION | K::METHOD) => "function",
        Some(K::CONSTRUCTOR) => "constructor",
        Some(K::CLASS | K::STRUCT | K::INTERFACE | K::TYPE_PARAMETER) => "type",
        Some(K::ENUM) => "type.enum",
        Some(K::ENUM_MEMBER) => "type.enum.variant",
        Some(K::MODULE) => "namespace",
        Some(K::PROPERTY | K::FIELD) => "variable.other.member",
        Some(K::CONSTANT | K::VALUE) => "constant",
        Some(K::KEYWORD) => "keyword",
        Some(K::OPERATOR) => "operator",
        Some(K::SNIPPET) => "special",
        Some(K::FILE | K::FOLDER) => "ui.text.directory",
        Some(K::TEXT) => "string",
        _ => "variable",
    }
}

fn syntax_ranges(
    source: &str,
    language: &str,
    loader: &syntax::Loader,
    theme: &Theme,
) -> Vec<(Range<usize>, Style)> {
    // Completion details are untrusted LSP data. Avoid parsing arbitrarily large
    // declarations or loading any parser for the generic kind-only fallback.
    if source.is_empty() || source.len() > 16 * 1024 {
        return Vec::new();
    }
    let rope = RopeSlice::from(source);
    let Some(language) = loader.language_for_name(language) else {
        return Vec::new();
    };
    let Ok(syntax) = Syntax::new(rope, language, loader) else {
        return Vec::new();
    };
    let mut highlighter = syntax.highlighter(rope, loader, ..);
    let mut highlights = Vec::new();
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < source.len() as u32 {
        if pos == highlighter.next_event_offset() {
            let (event, new) = highlighter.advance();
            if event == HighlightEvent::Refresh {
                highlights.clear();
            }
            highlights.extend(new);
        }
        let end = highlighter.next_event_offset().min(source.len() as u32);
        if end == pos {
            continue;
        }
        let style = highlights.iter().fold(Style::default(), |style, capture| {
            style.patch(theme.highlight(*capture))
        });
        result.push((pos as usize..end as usize, style));
        pos = end;
    }
    result
}

fn prepare(
    label: &format::Label,
    kind: Option<lsp::CompletionItemKind>,
    language: Option<&str>,
    loader: &syntax::Loader,
    theme: &Theme,
    width: usize,
) -> Prepared {
    let mut text = label.left.clone();
    let right_start = text.len() + 2;
    if !label.right.is_empty() {
        text.push_str("  ");
        text.push_str(&label.right);
    }
    let extra_start = text.len() + 2;
    if !label.extra.is_empty() {
        text.push_str("  ");
        text.push_str(&label.extra);
    }
    let mut ranges = vec![(
        0..label.left.len(),
        scope_style(theme, label.left_scope.unwrap_or(kind_scope(kind))),
    )];
    if !label.right.is_empty() {
        let scope = if label.right_is_metadata {
            "comment"
        } else {
            "type"
        };
        ranges.push((
            right_start..right_start + label.right.len(),
            theme.get(scope),
        ));
    }
    if !label.extra.is_empty() {
        ranges.push((extra_start..text.len(), theme.get("comment")));
    }
    if let Some(language) = language {
        for (source, style) in syntax_ranges(&label.source, language, loader, theme) {
            for projection in &label.projections {
                let start = source.start.max(projection.source.start);
                let end = source.end.min(projection.source.end);
                if start < end {
                    let target = projection.target.start + start - projection.source.start;
                    ranges.push((target..target + end - start, style));
                }
            }
        }
    }
    // The LSP knows the symbol's semantic kind even if its reconstructed
    // declaration is incomplete. Keep that color on the name itself; let the
    // parser supply the parameter/type/punctuation colors around it.
    if kind.is_some() {
        let chain = if label.left.starts_with("iter().") {
            "iter().".len()
        } else {
            0
        };
        let name_end = label.left[chain..]
            .find(['(', '<', ' '])
            .map_or(label.left.len(), |index| chain + index);
        let qualified = &label.left[chain..name_end];
        let name_start = qualified
            .rfind("::")
            .map(|index| index + 2)
            .or_else(|| qualified.rfind('.').map(|index| index + 1))
            .unwrap_or(0)
            + chain;
        ranges.push((
            name_start..name_end,
            scope_style(theme, label.left_scope.unwrap_or(kind_scope(kind))),
        ));
    }
    if let Some(start) = label.left.find('(').filter(|_| label.dim_arguments) {
        if let Some(end) = label.left.rfind(')') {
            ranges.push((start..end + 1, theme.get("comment")));
        }
    }
    if label.left.starts_with('•') {
        ranges.push((0..'•'.len_utf8(), theme.get("comment")));
    }
    let mut boundaries = vec![0, text.len()];
    for (range, _) in &ranges {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.retain(|&index| text.is_char_boundary(index));
    boundaries.sort_unstable();
    boundaries.dedup();
    let spans = Spans::from(
        boundaries
            .windows(2)
            .map(|range| {
                let (start, end) = (range[0], range[1]);
                let style = ranges
                    .iter()
                    .filter(|(range, _)| range.contains(&start))
                    .fold(Style::default(), |style, (_, overlay)| {
                        style.patch(*overlay)
                    });
                Span::styled(text[start..end].to_owned(), style)
            })
            .collect::<Vec<_>>(),
    );
    let mut left = slice(&spans, 0..label.left.len());
    let right = slice(&spans, right_start.min(text.len())..text.len());
    if right.width() > 0 {
        // Reserve a useful prefix of the name, but keep the type visible when a
        // long signature is truncated. Widths are terminal cells, not bytes.
        let name = label.left.split('(').next().unwrap_or(&label.left);
        let min_left = name.width().min(width / 2).max(1);
        let right_budget = right.width().min(width.saturating_sub(min_left + 2));
        let right = truncate(right, right_budget);
        left = truncate(left, width.saturating_sub(right.width() + 2));
        let gap = width.saturating_sub(left.width() + right.width());
        let left_bytes = text_len(&left);
        if !right.0.is_empty() {
            left.0.push(Span::raw(" ".repeat(gap)));
            left.0.extend(right.0);
        }
        Prepared {
            spans: left,
            left_bytes,
        }
    } else {
        let spans = truncate(left, width);
        let left_bytes = text_len(&spans);
        Prepared { spans, left_bytes }
    }
}

fn text_len(spans: &Spans<'_>) -> usize {
    spans.0.iter().map(|span| span.content.len()).sum()
}

fn slice(spans: &Spans<'_>, range: Range<usize>) -> Spans<'static> {
    let mut pos = 0;
    Spans::from(
        spans
            .0
            .iter()
            .filter_map(|span| {
                let start = range.start.saturating_sub(pos).min(span.content.len());
                let end = range.end.saturating_sub(pos).min(span.content.len());
                pos += span.content.len();
                (start < end).then(|| Span::styled(span.content[start..end].to_owned(), span.style))
            })
            .collect::<Vec<_>>(),
    )
}

fn truncate(spans: Spans<'static>, width: usize) -> Spans<'static> {
    if spans.width() <= width {
        return spans;
    }
    if width == 0 {
        return Spans::from(Vec::<Span>::new());
    }
    let mut result = Vec::new();
    let mut used = 0;
    'spans: for span in spans.0 {
        let mut text = String::new();
        for grapheme in span.content.graphemes(true) {
            if used + grapheme.width() > width - 1 {
                if !text.is_empty() {
                    result.push(Span::styled(text, span.style));
                }
                break 'spans;
            }
            text.push_str(grapheme);
            used += grapheme.width();
        }
        if !text.is_empty() {
            result.push(Span::styled(text, span.style));
        }
    }
    result.push(Span::raw("…"));
    Spans::from(result)
}

fn match_highlights(prepared: Prepared, filter: &str, theme: &Theme) -> Spans<'static> {
    if filter.is_empty() {
        return prepared.spans;
    }
    let left = slice(&prepared.spans, 0..prepared.left_bytes);
    let text: String = left.into();
    let pattern = Atom::new(
        filter,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );
    let mut indices = Vec::new();
    let mut buf = Vec::new();
    if pattern
        .indices(
            Utf32Str::new(&text, &mut buf),
            &mut MATCHER.lock(),
            &mut indices,
        )
        .is_none()
    {
        return prepared.spans;
    }
    indices.sort_unstable();
    let style = theme
        .try_get_exact("ui.menu.match")
        .unwrap_or_else(|| Style::default().add_modifier(Modifier::BOLD));
    let mut index = 0;
    let mut result: Vec<Span<'static>> = Vec::new();
    for span in prepared.spans.0 {
        for grapheme in span.content.graphemes(true) {
            let highlighted = indices.binary_search(&index).is_ok();
            let style = if highlighted {
                span.style.patch(style)
            } else {
                span.style
            };
            match result.last_mut() {
                Some(previous) if previous.style == style => {
                    previous.content.to_mut().push_str(grapheme)
                }
                _ => result.push(Span::styled(grapheme.to_owned(), style)),
            }
            index += 1;
        }
    }
    Spans::from(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        toml::from_str(
            r##"
            "function" = "#ff0000"
            "variable" = "#00ff00"
            "variable.parameter" = "#00ffff"
            "type" = "#0000ff"
            "type.builtin" = "#0000aa"
            "constant" = "#ff00ff"
            "keyword" = "#aaaa00"
            "comment" = "#888888"
            "punctuation" = "#aaaaaa"
            "ui.menu" = "#ffffff"
        "##,
        )
        .unwrap()
    }

    #[test]
    fn syntax_highlights_parameters_and_types_and_has_a_kind_fallback() {
        let theme = theme();
        let loader = helix_core::config::default_lang_loader();
        loader.set_scopes(theme.scopes().to_vec());
        let item: lsp::CompletionItem = serde_json::from_value(serde_json::json!({"label":"find(…)","kind":3,"detail":"fn find(value: i32) -> String"})).unwrap();
        let label = format::format(&item, Server::Rust);
        let prepared = prepare(&label, item.kind, Some("rust"), &loader, &theme, 60);
        let text: String = prepared.spans.clone().into();
        assert!(text.starts_with("find(value: i32)"));
        assert!(text.ends_with("String"));
        let colored = |needle: &str, style: Style| {
            prepared
                .spans
                .0
                .iter()
                .any(|span| span.content.contains(needle) && span.style.fg == style.fg)
        };
        assert!(colored("find", theme.get("function")));
        assert!(colored("value", theme.get("variable.parameter")));
        assert!(colored("i32", theme.get("type.builtin")));
        assert!(
            colored("String", theme.get("type.builtin")),
            "{:?}",
            prepared.spans
        );

        let item: lsp::CompletionItem = serde_json::from_value(serde_json::json!({
            "label":"find", "kind":3, "detail":"std::string", "labelDetails":{"detail":"(int value)"}
        })).unwrap();
        let label = format::format(&item, Server::Clang);
        let prepared = prepare(&label, item.kind, Some("cpp"), &loader, &theme, 60);
        let text: String = prepared.spans.clone().into();
        assert!(text.starts_with("find(int value)"));
        assert!(text.ends_with("std::string"));
        for (token, scope) in [
            ("find", "function"),
            ("int", "type.builtin"),
            ("value", "variable.parameter"),
            ("string", "type"),
        ] {
            assert!(
                prepared.spans.0.iter().any(
                    |span| span.content.contains(token) && span.style.fg == theme.get(scope).fg
                ),
                "{token}: {:?}",
                prepared.spans
            );
        }

        // An unknown server and a completely absent grammar still get colors.
        let empty = syntax::Loader::new(toml::from_str("language = []").unwrap()).unwrap();
        for (kind, scope) in [
            (3, "function"),
            (7, "type"),
            (6, "variable"),
            (21, "constant"),
            (14, "keyword"),
        ] {
            let item = serde_json::from_value(
                serde_json::json!({"label":"identifier","kind":kind,"detail":"source module"}),
            )
            .unwrap();
            let label = format::format(&item, Server::Other);
            let prepared = prepare(&label, item.kind, Some("not-installed"), &empty, &theme, 60);
            assert_eq!(prepared.spans.0[0].style.fg, theme.get(scope).fg);
            assert!(prepared
                .spans
                .0
                .iter()
                .any(|span| span.content.contains("source module")
                    && span.style.fg == theme.get("comment").fg));
        }

        // With no devicons configuration, the completion row still uses the
        // highlighter, and turning it off restores the plain label.
        use crate::handlers::completion::{CompletionItem, LspCompletionItem};
        use crate::ui::menu::Item;

        let item = CompletionItem::Lsp(LspCompletionItem {
            item: lsp::CompletionItem {
                label: "find".into(),
                kind: Some(lsp::CompletionItemKind::FUNCTION),
                ..Default::default()
            },
            provider: Default::default(),
            resolved: false,
            provider_priority: 0,
        });
        let mut style = super::super::CompletionStyle {
            directory: Style::default(),
            devicons: Arc::default(),
            theme: theme.clone(),
            completion_max_width: helix_view::editor::Config::default().completion_max_width,
            highlighter: Some(Highlighter {
                loader: Arc::new(ArcSwap::from_pointee(empty)),
                language: None,
                servers: HashMap::new(),
                cache: Mutex::default(),
                filter: Arc::default(),
            }),
        };
        let row = item.format(&style);
        let label = &row.cells[0].content.lines[0].0[0];
        assert_eq!(label.content, "find");
        assert_eq!(label.style.fg, theme.get("function").fg);
        assert_eq!(row.cells[1].content.lines[0].0[0].content, "function");
        style.highlighter = None;
        assert_eq!(
            item.format(&style).cells[0].content.lines[0].0[0].style,
            Style::default()
        );
    }

    #[test]
    fn truncation_keeps_types_and_unicode_boundaries_and_matches_keep_colors() {
        let theme = theme();
        let loader = syntax::Loader::new(toml::from_str("language = []").unwrap()).unwrap();
        let item: lsp::CompletionItem = serde_json::from_value(serde_json::json!({"label":"查找(…)","kind":3,"detail":"fn 查找(very_long_parameter: SomeLongType) -> String"})).unwrap();
        let label = format::format(&item, Server::Rust);
        for width in 1..65 {
            let prepared = prepare(&label, item.kind, None, &loader, &theme, width);
            assert!(prepared.spans.width() <= width, "width={width}");
            let text: String = prepared.spans.clone().into();
            if width >= 20 {
                assert!(text.ends_with("String"));
            }
            let spans = match_highlights(prepared, "查", &theme);
            assert_eq!(String::from(spans.clone()), text);
            if let Some(span) = spans.0.iter().find(|span| span.content == "查") {
                assert_eq!(span.style.fg, theme.get("function").fg);
                assert!(span.style.add_modifier.contains(Modifier::BOLD));
            }
        }
    }
}
