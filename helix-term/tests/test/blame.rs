use super::helpers::*;

use std::{path::Path, process::Command, sync::Arc, time::Duration};

use helix_core::{Selection, Transaction};
use helix_term::{application::Application, keymap::Keymaps, ui::EditorView};
use helix_view::{
    editor::{Action, InlineBlameShow},
    graphics::Rect,
};
use tui::buffer::Buffer;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_NAME", "Blame Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "Blame Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn wait_for_blame(app: &mut Application) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            run_event_loop_until_idle(app).await;
            if app
                .editor
                .documents()
                .filter(|doc| doc.path().is_some())
                .all(|doc| {
                    doc.file_blame
                        .as_ref()
                        .is_some_and(|blame| blame.text.is_instance(doc.text()))
                        && !doc.blame_controller.is_running()
                })
            {
                break;
            }
            app.editor.reset_idle_timer();
        }
    })
    .await
    .expect("blame did not settle");
}

#[tokio::test(flavor = "multi_thread")]
async fn background_blame_handles_multiple_documents_and_unsaved_changes() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let first = repo.path().join("first.txt");
    let second = repo.path().join("second.txt");
    std::fs::write(&first, "one\nkeep\n")?;
    std::fs::write(&second, "two\n")?;
    git(repo.path(), &["init"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    let mut config = test_config();
    config.editor.inline_blame.auto_fetch = true;
    config.editor.inline_blame.show = InlineBlameShow::CursorLine;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&first, None)
        .build()?;
    let second_id = app.editor.open(&second, Action::HorizontalSplit)?;
    wait_for_blame(&mut app).await;
    for doc in app.editor.documents().filter(|doc| doc.path().is_some()) {
        assert_eq!(
            doc.line_blame(0, "{author}: {title}")?,
            "Blame Author: initial"
        );
    }
    let view = app.editor.tree.focus;
    let doc = app.editor.document_mut(second_id).unwrap();
    let stale = doc.file_blame.as_ref().unwrap().clone();
    let transaction = Transaction::insert(doc.text(), &Selection::point(0), "new\n".into());
    doc.apply(&transaction, view);
    assert!(
        doc.line_blame(0, "{title}").is_err(),
        "stale annotations survived an edit"
    );
    wait_for_blame(&mut app).await;
    let doc = app.editor.document(second_id).unwrap();
    assert!(doc.line_blame(0, "{title}").is_err());
    assert_eq!(doc.line_blame(1, "{title}")?, "initial");
    assert!(Arc::ptr_eq(
        &stale.file,
        &doc.file_blame.as_ref().unwrap().file
    ));

    // An external commit must invalidate HEAD's snapshot on the next interaction.
    std::fs::write(&second, "two changed\n")?;
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "updated"]);
    app.editor.document_mut(second_id).unwrap().blame_last_check = None;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("j B"),
                Some(&|app: &Application| {
                    // The unsaved buffer is compared against the new HEAD, so its old
                    // text cannot incorrectly acquire the new commit's authorship.
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.line_blame(1, "{title}").is_err());
                    assert_eq!(
                        app.editor.get_status().unwrap().0.as_ref(),
                        "Not committed yet"
                    );
                }),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_blame_clips_to_scrolled_and_wrapped_viewports() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, format!("short\n\n{}\nlast\n", "x".repeat(70_000)))?;
    git(repo.path(), &["init"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    let mut config = test_config();
    config.editor.inline_blame.auto_fetch = true;
    config.editor.inline_blame.show = InlineBlameShow::AllLines;
    config.editor.inline_blame.format = "BLAME".into();
    let keys = Keymaps::new(Box::new(arc_swap::ArcSwap::from_pointee(
        config.keys.clone(),
    )));
    let app_view = EditorView::new(keys);
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;
    wait_for_blame(&mut app).await;
    let view_id = app.editor.tree.focus;
    app.editor.tree.get_mut(view_id).area = Rect::new(3, 2, 60, 9);
    for horizontal_offset in [0, 10, 65_535, 69_990] {
        let (_, doc) = helix_view::current!(app.editor);
        let mut offset = doc.view_offset(view_id);
        offset.horizontal_offset = horizontal_offset;
        doc.set_view_offset(view_id, offset);
        let (view, doc) = helix_view::current_ref!(app.editor);
        let mut surface = Buffer::empty(Rect::new(0, 0, 70, 15));
        surface.set_string(0, 0, "OUTSIDE", Default::default());
        app_view.render_view(&app.editor, doc, view, surface.area, &mut surface, true);
        let row = |y| {
            (3..63)
                .map(|x| surface[(x, y)].symbol.as_str())
                .collect::<String>()
        };
        if horizontal_offset == 0 {
            assert!(row(2).contains("BLAME"));
            assert!(
                !row(3).contains("BLAME"),
                "blank lines should not have annotations"
            );
            assert!(row(5).contains("BLAME"));
        }
        assert_eq!(&*surface[(0, 0)].symbol, "O");
        for y in 2..10 {
            for x in 64..70 {
                assert_eq!(&*surface[(x, y)].symbol, " ");
            }
        }
    }
    let mut wrapped_config = app.editor.config().clone();
    wrapped_config.soft_wrap.enable = Some(true);
    let (_, doc) = helix_view::current!(app.editor);
    doc.config = Arc::new(arc_swap::ArcSwap::from_pointee(wrapped_config));
    doc.set_view_offset(view_id, Default::default());
    let (view, doc) = helix_view::current_ref!(app.editor);
    let mut surface = Buffer::empty(Rect::new(0, 0, 70, 15));
    app_view.render_view(&app.editor, doc, view, surface.area, &mut surface, true);
    let rendered: String = surface
        .content
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect();
    assert_eq!(
        rendered.matches("BLAME").count(),
        1,
        "wrapped continuation rows duplicated blame"
    );
    app.editor.reset_idle_timer();
    test_key_sequence(&mut app, None, None, false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_blame_works_with_background_fetch_disabled() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "line\n")?;
    git(repo.path(), &["init"]);
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    assert!(helix_view::current_ref!(app.editor).1.file_blame.is_none());
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(" B"),
                Some(&|app: &Application| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert_eq!(
                        doc.line_blame(0, "{author}: {title}").unwrap(),
                        "Blame Author: initial"
                    );
                    assert!(app.editor.get_status().unwrap().0.contains("initial"));
                }),
            ),
            (
                Some(":set inline-blame.show cursor-line<ret>"),
                Some(&|app: &Application| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(
                        doc.file_blame.is_some(),
                        "a display toggle unnecessarily discarded cached blame"
                    );
                }),
            ),
            (
                Some(":set inline-blame.show never<ret>"),
                Some(&|app: &Application| {
                    let (_, doc) = helix_view::current_ref!(app.editor);
                    assert!(doc.file_blame.is_none());
                    assert!(!doc.blame_controller.is_running());
                }),
            ),
        ],
        false,
    )
    .await
}
