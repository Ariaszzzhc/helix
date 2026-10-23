//! File and symbol breadcrumbs, with one modal navigator for mouse and keyboard.

use std::{ops::Range, path::PathBuf, sync::Arc};

use helix_core::{
    fuzzy::fuzzy_match,
    unicode::{segmentation::UnicodeSegmentation, width::UnicodeWidthStr},
    Position, Selection,
};
use helix_view::{
    breadcrumb::{containing_chain, Symbol},
    editor::Action,
    graphics::{CursorKind, Rect, Style},
    input::{MouseButton, MouseEventKind},
    Document, DocumentId, Editor, View, ViewId,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

use crate::{
    compositor::{Component, Context, Event, EventResult},
    ctrl, key, shift,
    ui::Prompt,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Path(PathBuf),
    Symbol(Vec<usize>),
    Crumb(usize),
}

#[derive(Clone, Debug)]
struct Entry {
    label: String,
    target: Target,
    directory: bool,
    icon: Option<(String, Style)>,
}

impl AsRef<str> for Entry {
    fn as_ref(&self) -> &str {
        &self.label
    }
}

impl Entry {
    fn display_label(&self) -> String {
        match &self.icon {
            Some((glyph, _)) => format!("{glyph} {}", self.label),
            None => self.label.clone(),
        }
    }

    fn render(&self, label: &str, area: Rect, style: Style, surface: &mut Surface) {
        surface.set_stringn(area.x, area.y, label, area.width as usize, style);
        if let Some((glyph, icon_style)) = &self.icon {
            // A narrow segment may contain only an ellipsis. Never paint an
            // icon over that marker or split a wide glyph at the right edge.
            if label.starts_with(glyph) && glyph.width() <= area.width as usize {
                surface.set_stringn(
                    area.x,
                    area.y,
                    glyph,
                    area.width as usize,
                    style.patch(*icon_style),
                );
            }
        }
    }
}

fn set_icons(entries: &mut [Entry], editor: &Editor, doc: &Document, symbols: &[Symbol]) {
    let config = &editor.devicons;
    if !config.enable || !config.breadcrumb {
        return;
    }
    for entry in entries {
        let icon = match &entry.target {
            Target::Path(path) if entry.directory => config.directory(path, &editor.theme),
            Target::Path(path) => config.file(
                Some(path),
                if doc.path() == Some(path.as_path()) {
                    doc.language_name()
                } else {
                    None
                },
                &editor.theme,
            ),
            Target::Symbol(path) => {
                let Some(symbol) = symbol_at(symbols, path) else {
                    continue;
                };
                let kind = match symbol.kind.as_str() {
                    "type_parameter" => "type_param",
                    kind => kind,
                };
                config.kind(kind, &editor.theme)
            }
            Target::Crumb(_) => config.file(doc.path(), doc.language_name(), &editor.theme),
        };
        if !icon.glyph.is_empty() {
            entry.icon = Some((icon.glyph.to_owned(), icon.style));
        }
    }
}

fn symbol_at<'a>(roots: &'a [Symbol], path: &[usize]) -> Option<&'a Symbol> {
    let (first, rest) = path.split_first()?;
    let mut symbol = roots.get(*first)?;
    for index in rest {
        symbol = symbol.children.get(*index)?;
    }
    Some(symbol)
}

fn crumbs(editor: &Editor, view: &View) -> Vec<Entry> {
    let doc = &editor.documents[&view.doc];
    let config = &editor.config().breadcrumb;
    let mut result = Vec::new();
    if config.file_path {
        if let Some(path) = doc.path() {
            let root = doc.workspace_root();
            if let Ok(relative) = path.strip_prefix(root) {
                let mut current = root.to_path_buf();
                for component in relative.components() {
                    current.push(component);
                    result.push(Entry {
                        icon: None,
                        label: component.as_os_str().to_string_lossy().into_owned(),
                        target: Target::Path(current.clone()),
                        directory: current != path,
                    });
                }
            } else {
                // Preserve actual paths in targets, even when their labels are shortened.
                result.push(Entry {
                    icon: None,
                    label: helix_stdx::path::get_truncated_path(path)
                        .to_string_lossy()
                        .into_owned(),
                    target: Target::Path(path.to_path_buf()),
                    directory: false,
                });
            }
        } else {
            result.push(Entry {
                icon: None,
                label: doc.display_name().into_owned(),
                target: Target::Crumb(0),
                directory: false,
            });
        }
    }
    if config.symbols && doc.breadcrumb_version == Some(doc.version()) {
        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        let chain = containing_chain(&doc.breadcrumb_symbols, cursor);
        for depth in 1..=chain.len() {
            let path = chain[..depth].to_vec();
            let symbol = symbol_at(&doc.breadcrumb_symbols, &path).unwrap();
            result.push(Entry {
                icon: None,
                label: symbol.name.clone(),
                target: Target::Symbol(path),
                directory: false,
            });
        }
    }
    set_icons(&mut result, editor, doc, &doc.breadcrumb_symbols);
    result
}

#[derive(Debug)]
struct Segment {
    indices: Range<usize>,
    collapsed: bool,
    label: String,
    x: u16,
    width: u16,
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    if width == 0 {
        return result;
    }
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let size = grapheme.width();
        if used + size >= width {
            break;
        }
        result.push_str(grapheme);
        used += size;
    }
    result.push('…');
    result
}

/// The same display-cell geometry is used for rendering and hit testing.
fn layout(entries: &[Entry], area: Rect, focus: Option<usize>) -> Vec<Segment> {
    if entries.is_empty() || area.width == 0 {
        return Vec::new();
    }
    let file = entries
        .iter()
        .rposition(|entry| matches!(entry.target, Target::Path(_)) && !entry.directory);
    let mut visible = vec![true; entries.len()];
    let make_segments = |visible: &[bool]| {
        let mut result = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            let start = i;
            let label = if visible[i] {
                i += 1;
                entries[start].display_label()
            } else {
                while i < entries.len() && !visible[i] {
                    i += 1;
                }
                "…".to_owned()
            };
            result.push(Segment {
                indices: start..i,
                collapsed: !visible[start],
                label,
                x: 0,
                width: 0,
            });
        }
        result
    };
    let mut segments = make_segments(&visible);
    for index in 0..entries.len().saturating_sub(1) {
        let width: usize = segments
            .iter()
            .map(|segment| segment.label.width())
            .sum::<usize>()
            + 3 * segments.len().saturating_sub(1);
        if width <= area.width as usize {
            break;
        }
        if Some(index) != file && Some(index) != focus {
            visible[index] = false;
            segments = make_segments(&visible);
        }
    }
    // Tiny windows cannot fit even the separators. Show the focused/deepest
    // segment, and keep the rest accessible through keyboard navigation.
    if segments.len() * 4 - 3 > area.width as usize {
        let index = focus.unwrap_or(entries.len() - 1);
        let label = truncate(&entries[index].display_label(), area.width as usize);
        return vec![Segment {
            indices: index..index + 1,
            collapsed: false,
            width: label.width() as u16,
            label,
            x: area.x,
        }];
    }
    let budget = area.width as usize - 3 * segments.len().saturating_sub(1);
    let mut widths: Vec<_> = segments
        .iter()
        .map(|segment| segment.label.width().max(1))
        .collect();
    while widths.iter().sum::<usize>() > budget {
        let index = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, width)| *width)
            .unwrap()
            .0;
        widths[index] -= 1;
    }
    let mut x = area.x;
    for (segment, width) in segments.iter_mut().zip(widths) {
        segment.label = truncate(&segment.label, width);
        segment.x = x;
        segment.width = segment.label.width() as u16;
        x = x.saturating_add(segment.width).saturating_add(3);
    }
    segments
}

fn bar_area(view: &View) -> Rect {
    view.area.with_height(view.breadcrumb_height())
}

fn render_bar(
    editor: &Editor,
    entries: &[Entry],
    area: Rect,
    focus: Option<usize>,
    surface: &mut Surface,
) {
    if area.height == 0 {
        return;
    }
    let theme = &editor.theme;
    let style = theme
        .try_get("ui.breadcrumb")
        .unwrap_or_else(|| theme.get("ui.statusline"));
    let focused = theme
        .try_get("ui.breadcrumb.focused")
        .unwrap_or_else(|| theme.get("ui.menu.selected"));
    let separator = theme
        .try_get("ui.breadcrumb.separator")
        .unwrap_or_else(|| theme.get("ui.text.inactive"));
    surface.clear_with(area, style);
    let segments = layout(entries, area, focus);
    for (index, segment) in segments.iter().enumerate() {
        if index != 0 {
            surface.set_string(segment.x - 3, area.y, " > ", separator);
        }
        let style = if focus.is_some_and(|index| segment.indices.contains(&index)) {
            focused
        } else {
            style
        };
        let segment_area = Rect::new(segment.x, area.y, segment.width, 1);
        if segment.collapsed {
            surface.set_stringn(
                segment.x,
                area.y,
                &segment.label,
                segment.width as usize,
                style,
            );
        } else {
            entries[segment.indices.start].render(&segment.label, segment_area, style, surface);
        }
    }
}

pub(super) fn render(editor: &Editor, view: &View, surface: &mut Surface) {
    if view.breadcrumb_height() != 0 {
        render_bar(editor, &crumbs(editor, view), bar_area(view), None, surface);
    }
}

/// Returns a modal navigator only for a click on an actual breadcrumb segment.
pub(super) fn clicked(editor: &Editor, row: u16, column: u16) -> Option<Navigator> {
    for (view, _) in editor.tree.views() {
        if view.breadcrumb_height() == 0 || row != view.area.y {
            continue;
        }
        let entries = crumbs(editor, view);
        let segments = layout(&entries, bar_area(view), None);
        let Some(segment) = segments
            .into_iter()
            .find(|segment| column >= segment.x && column < segment.x + segment.width)
        else {
            continue;
        };
        let mut navigator = Navigator::new(editor, view.id)?;
        navigator.focus = segment.indices.start;
        if segment.collapsed {
            navigator.set_level(Level::Hidden(segment.indices), editor, None);
        } else {
            navigator.expand(editor);
        }
        return Some(navigator);
    }
    None
}

#[derive(Clone)]
enum Level {
    Directory(PathBuf),
    Symbols(Vec<usize>),
    Hidden(Range<usize>),
}

pub(crate) struct Navigator {
    pub(super) view: ViewId,
    doc: DocumentId,
    version: i32,
    symbols: Arc<Vec<Symbol>>,
    entries: Vec<Entry>,
    focus: usize,
    level: Option<Level>,
    options: Vec<Entry>,
    matches: Vec<Entry>,
    selected: usize,
    prompt: Prompt,
    filtering: bool,
    filter_selection: usize,
    popup: Rect,
    list: Rect,
}

impl Navigator {
    pub(crate) fn new(editor: &Editor, view: ViewId) -> Option<Self> {
        let view = editor.tree.get(view);
        let entries = crumbs(editor, view);
        if !editor.config().breadcrumb.visible() || entries.is_empty() {
            return None;
        }
        let doc = &editor.documents[&view.doc];
        Some(Self {
            view: view.id,
            doc: doc.id(),
            version: doc.version(),
            symbols: if doc.breadcrumb_version == Some(doc.version()) {
                doc.breadcrumb_symbols.clone()
            } else {
                Arc::default()
            },
            focus: entries.len() - 1,
            entries,
            level: None,
            options: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            filtering: false,
            filter_selection: 0,
            prompt: Prompt::new("/".into(), None, |_, _| Vec::new(), |_, _, _| {}),
            popup: Rect::default(),
            list: Rect::default(),
        })
    }

    fn valid(&self, editor: &Editor) -> bool {
        editor.config().breadcrumb.visible()
            && editor
                .tree
                .try_get(self.view)
                .is_some_and(|view| view.doc == self.doc)
            && editor
                .document(self.doc)
                .is_some_and(|doc| doc.version() == self.version)
    }

    fn set_level(&mut self, level: Level, editor: &Editor, selected: Option<&Target>) {
        let mut options: Vec<Entry> = match &level {
            Level::Directory(path) => super::directory_content(path, editor, false)
                .unwrap_or_default()
                .into_iter()
                .filter(|(entry, _)| entry != &path.join(".."))
                .map(|(entry, directory)| Entry {
                    icon: None,
                    label: format!(
                        "{}{}",
                        entry.file_name().unwrap_or_default().to_string_lossy(),
                        if directory { "/" } else { "" }
                    ),
                    target: Target::Path(entry),
                    directory,
                })
                .collect(),
            Level::Symbols(parent) => {
                let symbols = if parent.is_empty() {
                    &self.symbols
                } else {
                    let Some(symbol) = symbol_at(&self.symbols, parent) else {
                        return;
                    };
                    &symbol.children
                };
                symbols
                    .iter()
                    .enumerate()
                    .map(|(index, symbol)| {
                        let mut path = parent.clone();
                        path.push(index);
                        Entry {
                            icon: None,
                            label: format!("{}  {}", symbol.name, symbol.kind),
                            target: Target::Symbol(path),
                            directory: false,
                        }
                    })
                    .collect()
            }
            Level::Hidden(range) => range
                .clone()
                .map(|index| Entry {
                    icon: self.entries[index].icon.clone(),
                    label: self.entries[index].label.clone(),
                    target: Target::Crumb(index),
                    directory: false,
                })
                .collect(),
        };
        if let Some(target @ Target::Path(path)) = selected {
            // An explicitly opened file can be hidden by explorer filters. Keep
            // the current segment selectable without exposing other ignored files.
            if !options.iter().any(|entry| &entry.target == target) {
                options.insert(
                    0,
                    Entry {
                        icon: None,
                        label: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        target: Target::Path(path.clone()),
                        directory: path.is_dir(),
                    },
                );
            }
        }
        if !matches!(level, Level::Hidden(_)) {
            set_icons(
                &mut options,
                editor,
                &editor.documents[&self.doc],
                &self.symbols,
            );
        }
        self.level = Some(level);
        self.filtering = false;
        self.prompt.clear(editor);
        self.options = options;
        self.filter();
        self.selected = selected
            .and_then(|target| {
                self.matches
                    .iter()
                    .position(|entry| &entry.target == target)
            })
            .unwrap_or(0);
    }

    fn close_menu(&mut self) {
        self.level = None;
        self.filtering = false;
    }

    fn filter(&mut self) {
        self.matches = if self.prompt.line().is_empty() {
            self.options.clone()
        } else {
            fuzzy_match(self.prompt.line(), self.options.iter().cloned(), false)
                .into_iter()
                .map(|(entry, _)| entry)
                .collect()
        };
        self.selected = 0;
    }

    fn expand(&mut self, editor: &Editor) {
        let target = self.entries[self.focus].target.clone();
        let level = match &target {
            Target::Path(path) => path
                .parent()
                .map(|parent| Level::Directory(parent.to_path_buf())),
            Target::Symbol(path) => Some(Level::Symbols(path[..path.len() - 1].to_vec())),
            Target::Crumb(_) => Some(Level::Symbols(Vec::new())),
        };
        if let Some(level) = level {
            self.set_level(level, editor, Some(&target));
        }
    }

    fn descend(&mut self, editor: &Editor) {
        let Some(entry) = self.matches.get(self.selected).cloned() else {
            return;
        };
        match entry.target {
            Target::Path(path) if entry.directory => {
                self.set_level(Level::Directory(path), editor, None)
            }
            Target::Path(path)
                if editor.document(self.doc).and_then(Document::path) == Some(path.as_path()) =>
            {
                self.set_level(Level::Symbols(Vec::new()), editor, None);
            }
            Target::Symbol(path)
                if symbol_at(&self.symbols, &path)
                    .is_some_and(|symbol| !symbol.children.is_empty()) =>
            {
                self.set_level(Level::Symbols(path), editor, None);
            }
            Target::Crumb(index) => {
                self.focus = index;
                self.expand(editor);
            }
            _ => (),
        }
    }

    fn ascend(&mut self, editor: &Editor) {
        match self.level.clone() {
            Some(Level::Directory(path)) => {
                if let Some(parent) = path.parent() {
                    self.set_level(
                        Level::Directory(parent.to_path_buf()),
                        editor,
                        Some(&Target::Path(path)),
                    );
                }
            }
            Some(Level::Symbols(mut path)) if !path.is_empty() => {
                let selected = Target::Symbol(path.clone());
                path.pop();
                self.set_level(Level::Symbols(path), editor, Some(&selected));
            }
            Some(Level::Symbols(_)) => {
                if let Some(path) = editor.document(self.doc).and_then(Document::path) {
                    let selected = Target::Path(path.to_path_buf());
                    if let Some(parent) = path.parent() {
                        self.set_level(
                            Level::Directory(parent.to_path_buf()),
                            editor,
                            Some(&selected),
                        );
                    }
                } else {
                    self.close_menu();
                }
            }
            _ => self.close_menu(),
        }
    }

    fn confirm(&mut self, editor: &mut Editor) -> bool {
        let Some(entry) = self.matches.get(self.selected).cloned() else {
            return false;
        };
        match entry.target {
            Target::Path(_) if entry.directory => self.descend(editor),
            Target::Path(path) => {
                editor.focus(self.view);
                match editor.open(&path, Action::Replace) {
                    Ok(_) => return true,
                    Err(err) => editor.set_error(err.to_string()),
                }
            }
            Target::Symbol(path) => {
                let Some(symbol) = symbol_at(&self.symbols, &path) else {
                    return false;
                };
                editor.focus(self.view);
                let view = editor.tree.get_mut(self.view);
                let doc = editor.documents.get_mut(&self.doc).unwrap();
                view.push_jump(doc, (self.doc, doc.selection(self.view).clone()));
                doc.set_selection(self.view, Selection::point(symbol.selection));
                helix_view::align_view(doc, view, helix_view::Align::Center);
                return true;
            }
            Target::Crumb(index) => {
                self.focus = index;
                self.expand(editor);
            }
        }
        false
    }
}

fn close() -> EventResult {
    EventResult::Consumed(Some(Box::new(|compositor, _| {
        compositor.pop();
    })))
}

impl Component for Navigator {
    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        if !self.valid(cx.editor) {
            return close();
        }
        match event {
            Event::Key(event) => {
                if self.level.is_none() {
                    match *event {
                        key!('h') | key!(Left) => self.focus = self.focus.saturating_sub(1),
                        key!('l') | key!(Right) => {
                            self.focus = (self.focus + 1).min(self.entries.len() - 1)
                        }
                        key!('0') | key!(Home) => self.focus = 0,
                        key!('$') | key!(End) => self.focus = self.entries.len() - 1,
                        key!('j') | key!(Down) | key!(Enter) => self.expand(cx.editor),
                        key!(Esc) | ctrl!('c') => return close(),
                        _ => (),
                    }
                } else {
                    match *event {
                        key!(Esc) if self.filtering => {
                            self.filtering = false;
                            self.prompt.clear(cx.editor);
                            self.filter();
                            self.selected = self
                                .filter_selection
                                .min(self.matches.len().saturating_sub(1));
                        }
                        key!(Esc) => self.close_menu(),
                        ctrl!('c') => return close(),
                        key!('/') if !self.filtering => {
                            self.filtering = true;
                            self.filter_selection = self.selected;
                        }
                        key!('j') if !self.filtering => {
                            self.selected =
                                (self.selected + 1).min(self.matches.len().saturating_sub(1));
                        }
                        key!('k') if !self.filtering => {
                            self.selected = self.selected.saturating_sub(1)
                        }
                        ctrl!('n') | key!(Down) => {
                            self.selected =
                                (self.selected + 1).min(self.matches.len().saturating_sub(1))
                        }
                        ctrl!('p') | key!(Up) => self.selected = self.selected.saturating_sub(1),
                        key!(Tab) => self.descend(cx.editor),
                        shift!(Tab) => self.ascend(cx.editor),
                        key!(Enter) => {
                            if self.confirm(cx.editor) {
                                return close();
                            }
                        }
                        _ if self.filtering => {
                            let before = self.prompt.line().clone();
                            self.prompt.handle_event(&Event::Key(*event), cx);
                            if *self.prompt.line() != before {
                                self.filter();
                            }
                        }
                        _ => (),
                    }
                }
            }
            Event::Paste(_) => {
                if self.filtering {
                    self.prompt.handle_event(event, cx);
                    self.filter();
                }
            }
            Event::Mouse(event) if cx.editor.config().mouse => {
                let row = event.row;
                let column = event.column;
                match event.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let area = bar_area(cx.editor.tree.get(self.view));
                        if row == area.y {
                            if let Some(segment) = layout(&self.entries, area, Some(self.focus))
                                .into_iter()
                                .find(|segment| {
                                    column >= segment.x && column < segment.x + segment.width
                                })
                            {
                                self.focus = segment.indices.start;
                                if segment.collapsed {
                                    self.set_level(Level::Hidden(segment.indices), cx.editor, None);
                                } else {
                                    self.expand(cx.editor);
                                }
                            } else {
                                return close();
                            }
                        } else if self.level.is_some()
                            && self.list.intersects(Rect::new(column, row, 1, 1))
                        {
                            let start = self
                                .selected
                                .saturating_sub(self.list.height.saturating_sub(1) as usize);
                            self.selected = start + (row - self.list.y) as usize;
                            if self.confirm(cx.editor) {
                                return close();
                            }
                        } else if self.level.is_none()
                            || !self.popup.intersects(Rect::new(column, row, 1, 1))
                        {
                            return close();
                        }
                    }
                    MouseEventKind::ScrollDown if self.level.is_some() => {
                        self.selected =
                            (self.selected + 1).min(self.matches.len().saturating_sub(1))
                    }
                    MouseEventKind::ScrollUp if self.level.is_some() => {
                        self.selected = self.selected.saturating_sub(1)
                    }
                    _ => (),
                }
            }
            Event::Resize(..) => (),
            _ => return EventResult::Ignored(None),
        }
        EventResult::Consumed(None)
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        self.popup = Rect::default();
        self.list = Rect::default();
        if !self.valid(cx.editor) {
            return;
        }
        let bar = bar_area(cx.editor.tree.get(self.view));
        render_bar(cx.editor, &self.entries, bar, Some(self.focus), surface);
        let overhead = 2 + u16::from(self.filtering);
        let min_height = overhead + 1;
        if self.level.is_none() || area.width < 6 || area.height < min_height {
            return;
        }
        let segments = layout(&self.entries, bar, Some(self.focus));
        let x = segments
            .iter()
            .find(|segment| segment.indices.contains(&self.focus))
            .map_or(bar.x, |segment| segment.x);
        let width = area.width.min(72);
        let below = area.bottom().saturating_sub(bar.y + 1);
        let above = bar.y.saturating_sub(area.y);
        let open_below = below >= min_height || below >= above;
        let available = if open_below { below } else { above };
        if available < min_height {
            return;
        }
        let height = self
            .matches
            .len()
            .max(1)
            .saturating_add(overhead as usize)
            .min(available as usize) as u16;
        self.popup = Rect::new(
            x.min(area.right() - width).max(area.x),
            if open_below {
                bar.y + 1
            } else {
                bar.y - height
            },
            width,
            height,
        );
        let theme = &cx.editor.theme;
        let style = theme.get("ui.popup");
        surface.clear_with(self.popup, style);
        Block::bordered().style(style).render(self.popup, surface);
        let inner = self
            .popup
            .clip_left(1)
            .clip_right(1)
            .clip_top(1)
            .clip_bottom(1);
        self.list = inner.clip_top(u16::from(self.filtering));
        let selected_style = theme.get("ui.menu.selected");
        let start = self
            .selected
            .saturating_sub(self.list.height.saturating_sub(1) as usize);
        for (row, entry) in self
            .matches
            .iter()
            .skip(start)
            .take(self.list.height as usize)
            .enumerate()
        {
            let style = if start + row == self.selected {
                selected_style
            } else {
                style
            };
            let row_area = Rect::new(self.list.x, self.list.y + row as u16, self.list.width, 1);
            surface.set_style(row_area, style);
            entry.render(&entry.display_label(), row_area, style, surface);
        }
        if self.matches.is_empty() {
            surface.set_stringn(
                self.list.x,
                self.list.y,
                "No matches",
                self.list.width as usize,
                theme.get("ui.text.inactive"),
            );
        }
        if self.filtering {
            self.prompt.render_prompt(inner.with_height(1), surface, cx);
        }
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        if self.level.is_some() && self.popup.height != 0 {
            if self.filtering {
                let prompt_area = self
                    .popup
                    .clip_left(1)
                    .clip_right(1)
                    .clip_top(1)
                    .with_height(1);
                self.prompt.cursor(prompt_area, editor)
            } else {
                let row = self
                    .selected
                    .min(self.list.height.saturating_sub(1) as usize);
                (
                    Some(Position::new(
                        self.list.y as usize + row,
                        self.list.x as usize,
                    )),
                    CursorKind::Block,
                )
            }
        } else if let Some(view) = editor.tree.try_get(self.view) {
            let bar = bar_area(view);
            let segment = layout(&self.entries, bar, Some(self.focus))
                .into_iter()
                .find(|segment| segment.indices.contains(&self.focus));
            (
                segment
                    .filter(|_| bar.height != 0)
                    .map(|segment| Position::new(bar.y as usize, segment.x as usize)),
                CursorKind::Block,
            )
        } else {
            (None, CursorKind::Hidden)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_clips_unicode_and_preserves_folded_targets() {
        let mut entries: Vec<_> = [
            "src",
            "很长的目录",
            "button.ts",
            "IButtonStyles",
            "背景e\u{301}",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, label)| Entry {
            icon: None,
            label: label.into(),
            target: Target::Path(PathBuf::from(label)),
            directory: index < 2,
        })
        .collect();
        for icon in [None, Some(("界".to_owned(), Style::default()))] {
            for entry in &mut entries {
                entry.icon = icon.clone();
            }
            for width in 0..100 {
                for focus in [None, Some(0), Some(4)] {
                    let area = Rect::new(3, 2, width, 1);
                    let segments = layout(&entries, area, focus);
                    for segment in &segments {
                        assert!(segment.x >= area.x);
                        assert!(segment.x + segment.width <= area.right());
                        assert_eq!(segment.label.width(), segment.width as usize);
                        assert!(!segment.indices.is_empty());
                    }
                    if width >= 40 {
                        let covered: Vec<_> = segments
                            .iter()
                            .flat_map(|segment| segment.indices.clone())
                            .collect();
                        assert_eq!(covered, (0..entries.len()).collect::<Vec<_>>());
                    }
                }
            }
        }
        assert_eq!(truncate("界e\u{301}abc", 4), "界e\u{301}…");
    }
}
