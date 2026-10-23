use super::*;

use helix_term::{
    compositor::{Component, Context, Event},
    job::Jobs,
    keymap::Keymaps,
    ui::{self, EditorView},
};
use helix_view::{
    editor::StatusLineElement as E,
    graphics::{Color, Modifier, Rect, Style},
    input::parse_macro,
};
use tui::buffer::Buffer;

#[tokio::test(flavor = "multi_thread")]
async fn bufferline_preserves_style_and_clips_to_viewport() -> anyhow::Result<()> {
    let mut config = helpers::test_config();
    config.devicons = toml::from_str(indoc! {r#"
        enable = true
        [extension]
        rs = { glyph = '界', fg = '#00ff00' }
    "#})?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    let doc = app.editor.documents.values_mut().next().unwrap();
    doc.set_path(Some(std::path::Path::new("one.rs")));
    let original_theme = app.editor.theme.clone();
    app.editor.theme = helix_view::Theme::from(toml::from_str::<toml::Value>(indoc! {r##"
        "ui.bufferline.active" = { fg = "#ffffff", bg = "#112233", modifiers = ["reversed"] }
    "##})?);

    for width in 0..20 {
        let mut surface = Buffer::empty(Rect::new(0, 0, 30, 1));
        surface.set_string(0, 0, "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!", Style::default());
        EditorView::render_bufferline(&app.editor, Rect::new(3, 0, width, 1), &mut surface);
        for x in (0..3).chain(3 + width..30) {
            assert_eq!(&*surface[(x, 0)].symbol, "!", "width={width}, x={x}");
        }
        if width >= 4 {
            assert_eq!(&*surface[(4, 0)].symbol, "界");
            assert_eq!(surface[(4, 0)].fg, Color::Rgb(0, 255, 0));
            assert_eq!(surface[(4, 0)].bg, Color::Rgb(0x11, 0x22, 0x33));
            assert!(surface[(4, 0)].modifier.contains(Modifier::REVERSED));
        }
    }
    app.editor.devicons = std::sync::Arc::default();
    let mut surface = Buffer::empty(Rect::new(0, 0, 30, 1));
    EditorView::render_bufferline(&app.editor, surface.area, &mut surface);
    let text: String = surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect();
    assert!(text.starts_with(" one.rs "));
    assert!(!text.contains('界'));
    app.editor.theme = original_theme;
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_preserves_text_and_avoids_duplicate_icons() -> anyhow::Result<()> {
    for (elements, expected) in [
        (vec![E::FileName, E::FileType], " [scratch]  text"),
        (vec![E::FileType], " text"),
        (vec![E::FileBaseName], " [scratch]"),
        (vec![E::FileAbsolutePath], " [scratch]"),
    ] {
        let mut config = helpers::test_config();
        config.devicons.enable = true;
        config.editor.statusline.left = elements;
        config.editor.statusline.right.clear();
        let keys = Keymaps::new(Box::new(arc_swap::ArcSwap::from_pointee(
            config.keys.clone(),
        )));
        let app_view = EditorView::new(keys);
        let mut app = AppBuilder::new().with_config(config).build()?;
        let (view, doc) = helix_view::current_ref!(app.editor);
        let mut surface = Buffer::empty(Rect::new(0, 0, 120, 150));
        app_view.render_view(&app.editor, doc, view, surface.area, &mut surface, true);
        let row = view.area.bottom() - 1;
        let text: String = (0..surface.area.width)
            .map(|x| surface[(x, row)].symbol.as_str())
            .collect();
        assert_eq!(text.trim(), expected);
        test_key_sequence(&mut app, None, None, false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn file_picker_icons_do_not_participate_in_fuzzy_matching() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("example.rs");
    std::fs::write(&path, "")?;
    let mut config = helpers::test_config();
    config.devicons = toml::from_str(indoc! {r#"
        enable = true
        [extension]
        rs = 'ICON'
    "#})?;
    let mut app = AppBuilder::new().with_config(config).build()?;
    let mut picker = ui::file_picker(&app.editor, root.path().to_path_buf());
    let mut jobs = Jobs::new();
    let mut cx = Context {
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
    };
    let mut surface = Buffer::empty(Rect::new(0, 0, 80, 12));
    picker.render(surface.area, &mut surface, &mut cx);
    assert_eq!(picker.selection(), Some(&path));
    let text: String = surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect();
    assert!(text.contains("ICON"));
    for (input, expected) in [("ICON", None), ("<C-w>example", Some(&path))] {
        for key in parse_macro(input)? {
            picker.handle_event(&Event::Key(key), &mut cx);
        }
        picker.render(surface.area, &mut surface, &mut cx);
        assert_eq!(picker.selection(), expected, "query: {input}");
    }
    drop(picker);
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn highlighted_menus_keep_token_colors_on_the_selected_row() -> anyhow::Result<()> {
    use helix_term::ui::menu::{Cell, Item, Row};
    use tui::text::{Span, Spans};

    struct ColoredLabel;
    impl Item for ColoredLabel {
        type Data = ();
        fn format(&self, _: &()) -> Row<'_> {
            Row::new([Cell::from(Spans::from(vec![
                Span::styled(
                    "fi",
                    Style::default()
                        .fg(Color::Rgb(255, 0, 0))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("nd", Style::default().fg(Color::Rgb(255, 0, 0))),
            ]))])
        }
    }

    let mut app = AppBuilder::new().build()?;
    let original_theme = app.editor.theme.clone();
    app.editor.theme = helix_view::Theme::from(toml::from_str::<toml::Value>(indoc! {r##"
        "ui.menu" = { fg = "#333333", bg = "#eeeeee" }
        "ui.menu.selected" = { fg = "#00ff00", bg = "#112233" }
        "ui.text.focus" = { fg = "#00ff00", bg = "#112233" }
    "##})?);
    let mut jobs = Jobs::new();
    let mut cx = Context {
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
    };
    for preserve in [true, false] {
        let mut menu =
            ui::Menu::new(vec![ColoredLabel], (), |_, _, _| {}).preserve_highlight_colors(preserve);
        menu.move_down();
        let mut surface = Buffer::empty(Rect::new(0, 0, 12, 1));
        menu.required_size((12, 1));
        menu.render(surface.area, &mut surface, &mut cx);
        let offset = surface
            .content
            .iter()
            .position(|cell| cell.symbol.as_str() == "f")
            .unwrap();
        assert_eq!(
            surface.content[offset].fg,
            if preserve {
                Color::Rgb(255, 0, 0)
            } else {
                Color::Rgb(0, 255, 0)
            }
        );
        assert_eq!(surface.content[offset].bg, Color::Rgb(0x11, 0x22, 0x33));
        assert!(surface.content[offset].modifier.contains(Modifier::BOLD));
        assert!(surface.content[offset + 1]
            .modifier
            .contains(Modifier::BOLD));
        assert!(!surface.content[offset + 2]
            .modifier
            .contains(Modifier::BOLD));

        let mut picker = ui::Picker::new(
            [ui::PickerColumn::new("name", |_: &&str, _: &()| {
                Span::styled("find", Style::default().fg(Color::Rgb(255, 0, 0))).into()
            })],
            0,
            ["find"],
            (),
            |_, _, _| {},
        )
        .preserve_highlight_colors(preserve);
        let mut surface = Buffer::empty(Rect::new(0, 0, 40, 8));
        picker.render(surface.area, &mut surface, &mut cx);
        let cell = surface
            .content
            .iter()
            .find(|cell| &*cell.symbol == "f")
            .unwrap();
        assert_eq!(
            cell.fg,
            if preserve {
                Color::Rgb(255, 0, 0)
            } else {
                Color::Rgb(0, 255, 0)
            }
        );
        assert_eq!(cell.bg, Color::Rgb(0x11, 0x22, 0x33));
    }
    app.editor.theme = original_theme;
    test_key_sequence(&mut app, None, None, false).await
}
