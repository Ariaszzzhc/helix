use super::helpers::*;

use std::{path::Path, sync::Arc, time::Duration};

use helix_core::{unicode::width::UnicodeWidthStr, Selection, Transaction};
use helix_term::{
    application::Application,
    compositor::{Compositor, Context, Event},
    job::Jobs,
    keymap::Keymaps,
    ui::EditorView,
};
use helix_view::{
    editor::Action,
    graphics::Rect,
    input::{parse_macro, MouseButton, MouseEvent, MouseEventKind},
    keyboard::KeyModifiers,
    Editor,
};
use tui::buffer::Buffer;

const SOURCE: &str = "mod outer {\n    fn first() { let value = 1; }\n    fn second() {}\n}\n";

fn build(path: &Path) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.breadcrumb.enable = true;
    AppBuilder::new()
        .with_config(config)
        .with_file(path, None)
        .build()
}

fn compositor() -> Compositor {
    let mut compositor = Compositor::new(Rect::new(0, 0, 100, 24));
    compositor.push(Box::new(EditorView::new(Keymaps::new(Box::new(
        arc_swap::ArcSwap::from_pointee(helix_term::keymap::default()),
    )))));
    compositor
}

fn render(compositor: &mut Compositor, editor: &mut Editor, jobs: &mut Jobs) -> Buffer {
    let mut surface = Buffer::empty(compositor.size());
    compositor.render(
        surface.area,
        &mut surface,
        &mut Context {
            editor,
            jobs,
            scroll: None,
        },
    );
    surface
}

fn row(surface: &Buffer, y: u16) -> String {
    let mut text = String::new();
    let mut x = 0;
    while x < surface.area.width {
        let cell = &surface[(x, y)];
        text.push_str(cell.symbol.as_str());
        x += cell.width().max(1) as u16;
    }
    text
}

fn keys(compositor: &mut Compositor, editor: &mut Editor, jobs: &mut Jobs, input: &str) {
    for key in parse_macro(input).unwrap() {
        compositor.handle_event(
            &Event::Key(key),
            &mut Context {
                editor,
                jobs,
                scroll: None,
            },
        );
    }
}

fn click(compositor: &mut Compositor, editor: &mut Editor, jobs: &mut Jobs, x: u16, y: u16) {
    compositor.handle_event(
        &Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            row: y,
            column: x,
            modifiers: KeyModifiers::NONE,
        }),
        &mut Context {
            editor,
            jobs,
            scroll: None,
        },
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_browses_symbol_hierarchy_without_moving_until_confirmed() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join(".helix"))?;
    let path = root.path().join("one.rs");
    std::fs::write(&path, SOURCE)?;
    let mut app = build(&path)?;
    let view = app.editor.tree.focus;
    let doc_id = app.editor.tree.get(view).doc;
    let original = SOURCE.find("value").unwrap();
    app.editor
        .document_mut(doc_id)
        .unwrap()
        .set_selection(view, Selection::single(original, original + 1));
    let mut compositor = compositor();
    let mut jobs = Jobs::new();
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(row(&surface, 0).starts_with("one.rs > outer > first"));
    let cached = app
        .editor
        .document(doc_id)
        .unwrap()
        .breadcrumb_symbols
        .clone();

    keys(&mut compositor, &mut app.editor, &mut jobs, " ;<ret>j");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let (cursor, _) = compositor.cursor(surface.area, &app.editor);
    assert!(row(&surface, cursor.unwrap().row as u16).contains("second  function"));
    assert!(row(&surface, 2).contains("first  function"));
    assert!(!row(&surface, 2).contains('/'));
    keys(&mut compositor, &mut app.editor, &mut jobs, "k");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let (cursor, _) = compositor.cursor(surface.area, &app.editor);
    assert!(row(&surface, cursor.unwrap().row as u16).contains("first  function"));

    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        "<S-tab><tab>/second",
    );
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("one.rs > outer"));
    let (cursor, _) = compositor.cursor(surface.area, &app.editor);
    let cursor = cursor.expect("navigation filter should have a cursor");
    let prompt_row = row(&surface, cursor.row as u16);
    let prompt_start = prompt_row
        .find("/second")
        .expect("cursor must stay on the filter row");
    assert_eq!(
        cursor.col,
        prompt_row[..prompt_start].width() + "/second".len()
    );

    keys(&mut compositor, &mut app.editor, &mut jobs, "<C-u>jk");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let (cursor, _) = compositor.cursor(surface.area, &app.editor);
    assert!(row(&surface, cursor.unwrap().row as u16).contains("/jk"));

    assert_eq!(
        app.editor.document(doc_id).unwrap().selection(view),
        &Selection::single(original, original + 1)
    );
    keys(&mut compositor, &mut app.editor, &mut jobs, "<esc>");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let (cursor, _) = compositor.cursor(surface.area, &app.editor);
    assert!(row(&surface, cursor.unwrap().row as u16).contains("first  function"));
    assert!(!row(&surface, 2).contains('/'));
    keys(&mut compositor, &mut app.editor, &mut jobs, "<esc><esc>");
    assert_eq!(
        app.editor.document(doc_id).unwrap().selection(view),
        &Selection::single(original, original + 1)
    );
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        " ;<ret>/second<ret>",
    );
    assert_eq!(
        app.editor.document(doc_id).unwrap().selection(view),
        &Selection::single(
            SOURCE.find("second").unwrap(),
            SOURCE.find("second").unwrap() + 1
        )
    );
    keys(&mut compositor, &mut app.editor, &mut jobs, "<C-o>");
    assert_eq!(
        app.editor.document(doc_id).unwrap().selection(view),
        &Selection::single(original, original + 1)
    );
    assert!(Arc::ptr_eq(
        &cached,
        &app.editor.document(doc_id).unwrap().breadcrumb_symbols
    ));
    compositor.resize(Rect::new(0, 0, 21, 24));
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let folded = row(&surface, 0).find('…').expect("ancestor should fold") as u16;
    click(&mut compositor, &mut app.editor, &mut jobs, folded, 0);
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(row(&surface, 2).contains("outer"));
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        "<ret><tab>j<ret>",
    );
    assert_eq!(
        app.editor
            .document(doc_id)
            .unwrap()
            .selection(view)
            .primary()
            .cursor(app.editor.document(doc_id).unwrap().text().slice(..)),
        SOURCE.find("second").unwrap()
    );

    test_key_sequence(&mut app, Some(":qa!<ret>"), None, true).await
}

#[tokio::test(flavor = "multi_thread")]
async fn mouse_and_keyboard_share_file_navigation_and_split_geometry() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join(".helix"))?;
    std::fs::create_dir(root.path().join("src"))?;
    let first = root.path().join("src/one.rs");
    let second = root.path().join("src/two.rs");
    std::fs::write(&first, SOURCE)?;
    std::fs::write(&second, "fn other() {}\n")?;
    let mut app = build(&first)?;
    let first_view = app.editor.tree.focus;
    app.editor.open(&second, Action::VerticalSplit)?;
    let mut compositor = compositor();
    let mut jobs = Jobs::new();
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let view = app.editor.tree.get(first_view);
    let area = view.area;
    let inner = view.inner_area(&app.editor.documents[&view.doc]);
    assert_eq!(inner.y, area.y + 1);
    assert_eq!(view.inner_height(), area.height.saturating_sub(2) as usize);
    assert!(view
        .pos_at_screen_coords(&app.editor.documents[&view.doc], area.y, inner.x, true)
        .is_none());
    assert!(view
        .gutter_coords_at_screen_coords(area.y, area.x)
        .is_none());
    assert!(row(&surface, area.y).contains("src > one.rs"));
    assert!(row(&surface, area.y).contains("src > two.rs"));

    // Click the file segment in the inactive split. Filtering and confirming
    // must affect that split, while the other split keeps its document.
    click(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        area.x + 7,
        area.y,
    );
    assert_eq!(app.editor.tree.focus, first_view);
    keys(&mut compositor, &mut app.editor, &mut jobs, "/two");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let option_row = (area.y + 1..surface.area.bottom())
        .find(|&y| row(&surface, y).contains("two.rs"))
        .unwrap();
    let option_text = row(&surface, option_row);
    let option_column = option_text[..option_text.find("two.rs").unwrap()].width() as u16;
    click(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        option_column,
        option_row,
    );
    assert_eq!(
        app.editor.documents[&app.editor.tree.get(first_view).doc].path(),
        Some(second.as_path())
    );
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        " ;0<ret><tab>/one<ret>",
    );
    assert_eq!(
        app.editor.documents[&app.editor.tree.get(first_view).doc].path(),
        Some(first.as_path())
    );

    let second_view = app
        .editor
        .tree
        .views()
        .find(|(view, _)| view.id != first_view)
        .unwrap()
        .0
        .id;
    let second_area = app.editor.tree.get(second_view).area;
    click(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        second_area.x + 7,
        second_area.y,
    );
    assert_eq!(app.editor.tree.focus, second_view);
    keys(&mut compositor, &mut app.editor, &mut jobs, "<esc><esc>");
    app.editor.focus(first_view);

    // The click target is the same row that is painted, with no header offset
    // leaking into document or gutter coordinates.
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let view = app.editor.tree.get(first_view);
    let doc = &app.editor.documents[&view.doc];
    let inner = view.inner_area(doc);
    let expected = view
        .pos_at_screen_coords(doc, inner.y, inner.x + 4, true)
        .unwrap();
    click(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        inner.x + 4,
        inner.y,
    );
    assert_eq!(
        app.editor.documents[&app.editor.tree.get(first_view).doc].selection(first_view),
        &Selection::single(expected, expected + 1)
    );
    assert!(row(&surface, 0).contains("one.rs"));

    // Live config changes remove and restore the reserved row in every split.
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        ":set-option breadcrumb.enable false<ret>",
    );
    run_event_loop_until_idle(&mut app).await;
    for (view, _) in app.editor.tree.views() {
        assert_eq!(
            view.inner_area(&app.editor.documents[&view.doc]).y,
            view.area.y
        );
    }
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        ":set-option breadcrumb.enable true<ret>",
    );
    run_event_loop_until_idle(&mut app).await;
    for (view, _) in app.editor.tree.views() {
        assert_eq!(view.breadcrumb_height(), 1);
    }
    test_key_sequence(&mut app, Some(":qa!<ret>"), None, true).await
}

#[tokio::test(flavor = "multi_thread")]
async fn edited_symbols_are_invalidated_and_rebuilt_for_the_current_version() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let path = root.path().join("one.rs");
    std::fs::write(&path, SOURCE)?;
    let mut app = build(&path)?;
    let view = app.editor.tree.focus;
    let id = app.editor.tree.get(view).doc;
    let doc = app.editor.document_mut(id).unwrap();
    assert_eq!(doc.breadcrumb_version, Some(doc.version()));
    let stale = doc.breadcrumb_symbols.clone();
    let position = SOURCE.find("first").unwrap();
    let transaction = Transaction::change(
        doc.text(),
        [(position, position + 5, Some("renamed".into()))].into_iter(),
    );
    doc.apply(&transaction, view);
    assert_eq!(doc.breadcrumb_version, None);
    let mut compositor = compositor();
    let mut jobs = Jobs::new();
    let before = app
        .editor
        .document(id)
        .unwrap()
        .selection(view)
        .primary()
        .cursor(app.editor.document(id).unwrap().text().slice(..));
    keys(&mut compositor, &mut app.editor, &mut jobs, " ;<ret><tab>");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("No matches"));
    keys(&mut compositor, &mut app.editor, &mut jobs, "<ret><C-c>");
    assert_eq!(
        app.editor
            .document(id)
            .unwrap()
            .selection(view)
            .primary()
            .cursor(app.editor.document(id).unwrap().text().slice(..)),
        before
    );

    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            run_event_loop_until_idle(&mut app).await;
            let doc = app.editor.document(id).unwrap();
            if doc.breadcrumb_version == Some(doc.version()) {
                break;
            }
            app.editor.reset_idle_timer();
        }
    })
    .await;
    if settled.is_err() {
        test_key_sequence(&mut app, Some(":qa!<ret>"), None, true).await?;
        anyhow::bail!("breadcrumb refresh did not settle");
    }
    let doc = app.editor.document(id).unwrap();
    assert!(!Arc::ptr_eq(&stale, &doc.breadcrumb_symbols));
    assert_eq!(doc.breadcrumb_symbols[0].children[0].name, "renamed");
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":set-language text<ret>"),
                Some(&|app: &Application| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.breadcrumb_symbols.is_empty());
                }),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn devicons_preserve_styles_hit_targets_and_text_filtering() -> anyhow::Result<()> {
    use helix_view::graphics::{Color, Modifier};

    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join(".helix"))?;
    std::fs::create_dir(root.path().join("src"))?;
    let path = root.path().join("src/one.rs");
    std::fs::write(&path, SOURCE)?;
    let mut config = test_config();
    config.editor.breadcrumb.enable = true;
    config.devicons = toml::from_str(
        r##"
        enable = true
        breadcrumb = true
        [directory]
        src = { glyph = "D", fg = "#ff0000" }
        [extension]
        rs = { glyph = "界", fg = "#00ff00" }
        [kind]
        module = "M"
        function = { glyph = "λ", fg = "#0000ff" }
    "##,
    )?;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    let view = app.editor.tree.focus;
    let id = app.editor.tree.get(view).doc;
    let original = SOURCE.find("value").unwrap();
    app.editor
        .document_mut(id)
        .unwrap()
        .set_selection(view, Selection::single(original, original + 1));
    let original_theme = app.editor.theme.clone();
    app.editor
        .set_theme(helix_view::Theme::from(toml::from_str::<toml::Value>(
            r##"
        "ui.selection" = { bg = "#333333" }
        "ui.breadcrumb" = { fg = "#ffffff", bg = "#112233" }
        "ui.breadcrumb.focused" = { bg = "#445566", modifiers = ["bold"] }
    "##,
        )?))?;
    let mut compositor = compositor();
    let mut jobs = Jobs::new();
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let bar = row(&surface, 0);
    assert!(bar.starts_with("D src > 界 one.rs > M outer > λ first"));
    for (glyph, color) in [
        ("D", Color::Rgb(255, 0, 0)),
        ("界", Color::Rgb(0, 255, 0)),
        ("λ", Color::Rgb(0, 0, 255)),
    ] {
        let x = bar[..bar.find(glyph).unwrap()].width() as u16;
        assert_eq!(surface[(x, 0)].fg, color);
        assert_eq!(surface[(x, 0)].bg, Color::Rgb(0x11, 0x22, 0x33));
    }
    // Clicking the second display cell of a wide icon opens the file menu.
    let file_x = bar[..bar.find('界').unwrap()].width() as u16;
    click(&mut compositor, &mut app.editor, &mut jobs, file_x + 1, 0);
    keys(&mut compositor, &mut app.editor, &mut jobs, "/界");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("No matches"));
    keys(
        &mut compositor,
        &mut app.editor,
        &mut jobs,
        "<C-u>one<ret> ;<ret>/λ",
    );
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>()
        .contains("No matches"));
    let bar = row(&surface, 0);
    let symbol_x = bar[..bar.find('λ').unwrap()].width() as u16;
    assert_eq!(surface[(symbol_x, 0)].fg, Color::Rgb(0, 0, 255));
    assert_eq!(surface[(symbol_x, 0)].bg, Color::Rgb(0x44, 0x55, 0x66));
    assert!(surface[(symbol_x, 0)].modifier.contains(Modifier::BOLD));
    keys(&mut compositor, &mut app.editor, &mut jobs, "<C-u>second");
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    let y = (1..surface.area.height)
        .find(|&y| row(&surface, y).contains("λ second"))
        .unwrap();
    let text = row(&surface, y);
    let x = text[..text.find('λ').unwrap()].width() as u16;
    assert_eq!(surface[(x, y)].fg, Color::Rgb(0, 0, 255));
    click(&mut compositor, &mut app.editor, &mut jobs, x, y);
    let doc = app.editor.document(id).unwrap();
    assert_eq!(
        doc.selection(view).primary().cursor(doc.text().slice(..)),
        SOURCE.find("second").unwrap()
    );

    let enabled = (*app.editor.devicons).clone();
    for global in [true, false] {
        let mut config = enabled.clone();
        config.enable = global;
        config.breadcrumb = !global;
        app.editor.devicons = Arc::new(config);
        let surface = render(&mut compositor, &mut app.editor, &mut jobs);
        assert!(row(&surface, 0).starts_with("src > one.rs > outer > second"));
    }
    let mut monochrome = enabled.clone();
    monochrome.color = false;
    app.editor.devicons = Arc::new(monochrome);
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert_eq!(surface[(file_x, 0)].fg, Color::Rgb(255, 255, 255));
    let mut hidden = enabled;
    hidden.extension.insert(
        "rs".into(),
        helix_view::devicons::IconConfig::Glyph(String::new()),
    );
    app.editor.devicons = Arc::new(hidden);
    let surface = render(&mut compositor, &mut app.editor, &mut jobs);
    assert!(row(&surface, 0).starts_with("D src > one.rs > M outer > λ second"));
    app.editor.set_theme(original_theme)?;
    test_key_sequence(&mut app, Some(":qa!<ret>"), None, true).await
}
