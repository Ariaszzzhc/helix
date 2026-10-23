use super::*;

use helix_term::{application::Application, keymap::Keymaps, ui::EditorView};
use helix_view::graphics::Rect;
use tui::buffer::Buffer;

fn render(app: &Application) -> String {
    let keys = Keymaps::new(Box::new(arc_swap::ArcSwap::from_pointee(
        helix_term::keymap::default(),
    )));
    let editor_view = EditorView::new(keys);
    let mut surface = Buffer::empty(Rect::new(0, 0, 120, 150));
    for (view, focused) in app.editor.tree.views() {
        let doc = &app.editor.documents[&view.doc];
        editor_view.render_view(&app.editor, doc, view, surface.area, &mut surface, focused);
    }
    surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn welcome_is_rendered_in_each_split_and_disappears_after_editing() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    assert!(render(&app).contains(":tutor"));
    // The welcome screen is a decoration, never document contents.
    assert_eq!(
        app.editor.documents().next().unwrap().text().to_string(),
        "\n"
    );
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":vsplit<ret>:hsplit<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.tree.views().count(), 3);
                    assert_eq!(render(app).matches(":tutor").count(), 3);
                }),
            ),
            (
                Some("ihello<esc>"),
                Some(&|app| {
                    assert!(!render(app).contains(":tutor"));
                }),
            ),
            (
                Some(":new<ret>"),
                Some(&|app| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(!doc.is_welcome);
                    assert!(!render(app).contains(":tutor"));
                }),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn welcome_respects_config_and_explicit_file_arguments() -> anyhow::Result<()> {
    let mut config = helpers::test_config();
    config.editor.welcome_screen = false;
    let mut app = AppBuilder::new().with_config(config).build()?;
    assert!(!render(&app).contains(":tutor"));
    test_key_sequence(&mut app, None, None, false).await?;

    let file = tempfile::NamedTempFile::new()?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let (_, doc) = helix_view::current_ref!(app.editor);
    assert!(!doc.is_welcome);
    assert!(!render(&app).contains(":tutor"));
    test_key_sequence(&mut app, None, None, false).await
}
