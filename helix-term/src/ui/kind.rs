//! Shared semantic styles for symbol and completion kinds.

use helix_view::{graphics::Style, Theme};

pub(crate) fn scope_for_name(kind: &str) -> &'static str {
    match kind {
        "function" | "method" => "function",
        "constructor" => "constructor",
        "class" | "interface" | "struct" | "type_param" | "type" => "type",
        "enum" => "type.enum",
        "enum_member" => "type.enum.variant",
        "module" | "namespace" | "package" => "namespace",
        "field" | "property" => "variable.other.member",
        "constant" | "value" => "constant",
        "keyword" => "keyword",
        "operator" => "operator",
        "snippet" => "special",
        "folder" | "file" => "ui.text.directory",
        "string" | "text" | "word" => "string",
        "number" => "constant.numeric",
        "boolean" | "null" => "constant.builtin",
        "macro" => "function.macro",
        "section" => "markup.heading",
        "key" => "variable.other.member",
        _ => "variable",
    }
}

pub(crate) fn scope_style(theme: &Theme, scope: &str) -> Style {
    theme
        .try_get(scope)
        .or_else(|| theme.try_get("variable"))
        .unwrap_or_default()
}

#[derive(Debug)]
pub(crate) struct SymbolStyle {
    pub devicons: std::sync::Arc<helix_view::devicons::Config>,
    pub theme: Theme,
}

impl SymbolStyle {
    pub fn new(editor: &helix_view::Editor) -> Self {
        Self {
            devicons: editor.devicons.clone(),
            theme: editor.theme.clone(),
        }
    }

    pub fn scope(&self, kind: &str) -> Style {
        scope_style(&self.theme, scope_for_name(kind))
    }

    pub fn icons(&self) -> bool {
        self.devicons.enable && self.devicons.picker
    }

    pub fn kind_span(&self, kind: &str, text: &'static str) -> tui::text::Span<'static> {
        if self.icons() {
            let icon_kind = match kind {
                "type" => "type_param",
                "macro" => "function",
                "section" => "module",
                _ => kind,
            };
            let mut icon = self.devicons.kind(icon_kind, &self.theme);
            if self.devicons.color && icon.style.fg.is_none() {
                icon.style = self.scope(kind).patch(icon.style);
            }
            super::icon_span(icon)
        } else {
            tui::text::Span::styled(text, self.scope(kind))
        }
    }
}
