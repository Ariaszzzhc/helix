//! Document symbols shared by breadcrumb views and their navigation menus.

use std::ops::Range;

use helix_core::{syntax::Loader, syntax::QueryMatchIterEvent, Rope, Syntax};
use helix_lsp::{lsp, util::lsp_pos_to_pos, OffsetEncoding};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    pub enable: bool,
    pub file_path: bool,
    pub symbols: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enable: false,
            file_path: true,
            symbols: true,
        }
    }
}

impl Config {
    pub fn visible(&self) -> bool {
        self.enable && (self.file_path || self.symbols)
    }
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// The complete definition, in character offsets (end exclusive).
    pub range: Range<usize>,
    /// The name of the definition, used as the jump target.
    pub selection: usize,
    pub children: Vec<Symbol>,
}

/// Find the deepest containing chain. Siblings may overlap in server responses;
/// prefer the smallest enclosing range rather than combining unrelated branches.
pub fn containing_chain(symbols: &[Symbol], cursor: usize) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut level = symbols;
    while let Some((index, symbol)) = level
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.range.contains(&cursor))
        .min_by_key(|(_, symbol)| symbol.range.len())
    {
        chain.push(index);
        level = &symbol.children;
    }
    chain
}

/// Construct a hierarchy only from strict range containment. Equal or crossing
/// ranges are siblings; names and `containerName` are not proof of parentage.
fn nest(mut symbols: Vec<Symbol>) -> Vec<Symbol> {
    symbols.sort_by_key(|symbol| (symbol.range.start, std::cmp::Reverse(symbol.range.end)));
    fn insert(level: &mut Vec<Symbol>, symbol: Symbol) {
        if let Some(parent) = level.last_mut().filter(|parent| {
            parent.range != symbol.range
                && parent.range.start <= symbol.range.start
                && symbol.range.end <= parent.range.end
        }) {
            insert(&mut parent.children, symbol);
        } else {
            level.push(symbol);
        }
    }
    let mut roots = Vec::new();
    for symbol in symbols {
        insert(&mut roots, symbol);
    }
    roots
}

pub fn from_syntax(syntax: &Syntax, loader: &Loader, text: &Rope) -> Vec<Symbol> {
    let text = text.slice(..);
    let mut tags = syntax.tags(text, loader, ..);
    let mut symbols = Vec::new();
    while let Some(event) = tags.next() {
        let QueryMatchIterEvent::Match(mat) = event else {
            continue;
        };
        let query = &loader.tag_query(tags.current_language()).unwrap().query;
        let mut definition = None;
        let mut name = None;
        for capture in mat.nodes.iter() {
            let capture_name = query.capture_name(capture.capture);
            if let Some(kind) = capture_name.strip_prefix("definition.") {
                definition = Some((kind, capture.node.byte_range()));
            } else if capture_name == "name" {
                name = Some(capture.node.byte_range());
            }
        }
        let Some((kind, range)) = definition else {
            continue;
        };
        let name = name.unwrap_or_else(|| range.clone());
        let selection = text.byte_to_char(name.start as usize);
        symbols.push(Symbol {
            name: text
                .slice(selection..text.byte_to_char(name.end as usize))
                .to_string(),
            kind: kind.to_owned(),
            range: text.byte_to_char(range.start as usize)..text.byte_to_char(range.end as usize),
            selection,
            children: Vec::new(),
        });
    }
    nest(symbols)
}

pub fn from_lsp(
    response: lsp::DocumentSymbolResponse,
    text: &Rope,
    encoding: OffsetEncoding,
    syntax_symbols: &[Symbol],
) -> Vec<Symbol> {
    fn convert(
        symbol: lsp::DocumentSymbol,
        text: &Rope,
        encoding: OffsetEncoding,
    ) -> Option<Symbol> {
        let start = lsp_pos_to_pos(text, symbol.range.start, encoding)?;
        let end = lsp_pos_to_pos(text, symbol.range.end, encoding)?;
        let selection = lsp_pos_to_pos(text, symbol.selection_range.start, encoding)?;
        if start >= end || !(start..end).contains(&selection) {
            return None;
        }
        Some(Symbol {
            name: symbol.name,
            kind: format!("{:?}", symbol.kind).to_lowercase(),
            range: start..end,
            selection,
            children: symbol
                .children
                .into_iter()
                .flatten()
                .filter_map(|child| convert(child, text, encoding))
                .filter(|child| start <= child.range.start && child.range.end <= end)
                .collect(),
        })
    }
    match response {
        lsp::DocumentSymbolResponse::Nested(symbols) => symbols
            .into_iter()
            .filter_map(|symbol| convert(symbol, text, encoding))
            .collect(),
        // Treat only strict range containment as hierarchy. A name-only location
        // can identify the symbol at its name, but never its surrounding body.
        lsp::DocumentSymbolResponse::Flat(symbols) => {
            fn flatten(symbols: Vec<Symbol>, out: &mut Vec<Symbol>) {
                for mut symbol in symbols {
                    let children = std::mem::take(&mut symbol.children);
                    out.push(symbol);
                    flatten(children, out);
                }
            }
            let mut syntax = Vec::new();
            flatten(syntax_symbols.to_vec(), &mut syntax);
            let mut symbols: Vec<_> = symbols
                .into_iter()
                .filter_map(|symbol| {
                    let start = lsp_pos_to_pos(text, symbol.location.range.start, encoding)?;
                    let end = lsp_pos_to_pos(text, symbol.location.range.end, encoding)?;
                    (start < end).then(|| Symbol {
                        name: symbol.name,
                        kind: format!("{:?}", symbol.kind).to_lowercase(),
                        range: start..end,
                        selection: start,
                        children: Vec::new(),
                    })
                })
                .collect();
            for symbol in &mut symbols {
                // Match the same definition by both name and position. Syntax
                // supplies its name target and expands name-only LSP locations;
                // only resulting range containment establishes parentage.
                if let Some(index) = syntax.iter().position(|syntax| {
                    syntax.name == symbol.name && symbol.range.contains(&syntax.selection)
                }) {
                    let syntax = syntax.remove(index);
                    symbol.selection = syntax.selection;
                    if syntax.range.start <= symbol.range.start
                        && symbol.range.end <= syntax.range.end
                    {
                        symbol.range = syntax.range;
                    }
                }
            }
            symbols.extend(syntax);
            nest(symbols)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, range: Range<usize>) -> Symbol {
        Symbol {
            name: name.into(),
            kind: "function".into(),
            selection: range.start,
            range,
            children: Vec::new(),
        }
    }

    #[test]
    fn containment_respects_boundaries_and_overlapping_siblings() {
        let symbols = nest(vec![
            symbol("child", 5..10),
            symbol("outer", 0..30),
            symbol("crossing", 8..20),
            symbol("next", 30..40),
        ]);
        assert_eq!(containing_chain(&symbols, 5), [0, 0]);
        assert_eq!(containing_chain(&symbols, 9), [0, 0]);
        assert_eq!(containing_chain(&symbols, 10), [0, 1]);
        assert_eq!(containing_chain(&symbols, 20), [0]);
        assert_eq!(containing_chain(&symbols, 30), [1]);
        assert!(containing_chain(&symbols, 40).is_empty());
        let equal = nest(vec![symbol("one", 0..10), symbol("two", 0..10)]);
        assert_eq!(equal.len(), 2);
        assert!(equal.iter().all(|symbol| symbol.children.is_empty()));
    }

    #[test]
    fn lsp_ranges_use_character_offsets_and_name_jump_targets() {
        // The emoji occupies two UTF-16 units, four UTF-8 bytes, and one char.
        let text = Rope::from_str("😀 class 界 { field }\n");
        let response: lsp::DocumentSymbolResponse = serde_json::from_value(serde_json::json!([{
            "name": "界", "kind": 5,
            "range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 20}},
            "selectionRange": {"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 10}},
            "children": [{
                "name": "field", "kind": 8,
                "range": {"start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 18}},
                "selectionRange": {"start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 18}}
            }]
        }])).unwrap();
        let roots = from_lsp(response, &text, OffsetEncoding::Utf16, &[]);
        assert_eq!(roots[0].range, 2..19);
        assert_eq!(roots[0].selection, 8);
        assert_eq!(roots[0].children[0].selection, 12);
        assert_eq!(containing_chain(&roots, 15), [0, 0]);
        assert_eq!(containing_chain(&roots, 17), [0]);
        let flat: lsp::DocumentSymbolResponse = serde_json::from_value(serde_json::json!([
            {"name": "界", "kind": 5, "location": {
                "uri": "file:///example.ts", "range": {
                    "start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 10}
                }
            }},
            {"name": "field", "kind": 8, "containerName": "界", "location": {
                "uri": "file:///example.ts", "range": {
                    "start": {"line": 0, "character": 13}, "end": {"line": 0, "character": 18}
                }
            }}
        ]))
        .unwrap();
        let flat_symbols = flat.clone();
        let flat = from_lsp(flat, &text, OffsetEncoding::Utf16, &[]);
        assert_eq!(flat.len(), 2);
        assert!(flat.iter().all(|symbol| symbol.children.is_empty()));
        assert_eq!(containing_chain(&flat, 15), [1]);
        assert!(containing_chain(&flat, 18).is_empty());
        let syntax = Symbol {
            children: Vec::new(),
            ..roots[0].clone()
        };
        let merged = from_lsp(flat_symbols, &text, OffsetEncoding::Utf16, &[syntax]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].selection, 8);
        assert_eq!(containing_chain(&merged, 15), [0, 0]);
    }
}
