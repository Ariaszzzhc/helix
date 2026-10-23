use std::{collections::HashMap, sync::Arc, time::Duration};

use helix_event::{cancelable_future, register_hook, send_blocking, AsyncHook};
use helix_loader::workspace_trust::TrustQuery;
use helix_vcs::{FileBlame, MappedBlame};
use helix_view::{
    editor::InlineBlameShow,
    events::{ConfigDidChange, DocumentDidChange, DocumentDidOpen},
    handlers::{BlameEvent, BlameQuery, Handlers},
    Document, DocumentId, Editor,
};
use tokio::{sync::Semaphore, time::Instant};

use crate::{events::PostCommand, job};

pub(super) struct BlameHandler {
    requests: HashMap<DocumentId, Option<BlameQuery>>,
    workers: Arc<Semaphore>,
}

impl Default for BlameHandler {
    fn default() -> Self {
        Self {
            requests: HashMap::new(),
            workers: Arc::new(Semaphore::new(2)),
        }
    }
}

impl AsyncHook for BlameHandler {
    type Event = BlameEvent;

    fn handle_event(&mut self, event: BlameEvent, _timeout: Option<Instant>) -> Option<Instant> {
        let request = self.requests.entry(event.doc_id).or_default();
        // A background refresh must not swallow a manual query in the same batch.
        if event.line.is_some() {
            *request = event.line;
        }
        Some(Instant::now() + Duration::from_millis(150))
    }

    fn finish_debounce(&mut self) {
        let requests = std::mem::take(&mut self.requests);
        let workers = self.workers.clone();
        job::dispatch_blocking(move |editor, _| {
            for (doc_id, line) in requests {
                request_blame(editor, doc_id, line, workers.clone());
            }
        });
    }
}

fn enabled(doc: &Document) -> bool {
    let config = doc.config.load();
    config.inline_blame.auto_fetch
        || (config.inline_blame.show != InlineBlameShow::Never && doc.file_blame.is_some())
}

fn request_blame(
    editor: &mut Editor,
    doc_id: DocumentId,
    line: Option<BlameQuery>,
    workers: Arc<Semaphore>,
) {
    let Some(doc) = editor.document(doc_id) else {
        return;
    };
    if line.is_none() && (!enabled(doc) || doc.blame_controller.is_running()) {
        return;
    }
    let Some(path) = doc.path().map(ToOwned::to_owned) else {
        return;
    };
    if line
        .as_ref()
        .is_some_and(|query| query.path != path || query.version != doc.version())
    {
        return;
    }
    let trust_full = editor
        .workspace_trust
        .query(doc.workspace_root(), TrustQuery::Git)
        .is_trusted();
    let doc = editor.document_mut(doc_id).unwrap();
    if trust_full != doc.blame_trust_full {
        doc.clear_blame();
    }
    doc.blame_trust_full = trust_full;
    doc.blame_last_check = Some(std::time::Instant::now());
    let cancel = doc.blame_controller.restart();
    let text = doc.text().clone();
    let encoding = doc.encoding();
    let previous = doc.file_blame.clone();

    tokio::spawn(async move {
        let Some(Ok(permit)) = cancelable_future(workers.acquire_owned(), &cancel).await else {
            return;
        };
        let task_cancel = cancel.clone();
        let task_path = path.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Keep the permit until the computation actually finishes: dropping
            // a spawn_blocking future cannot interrupt gix's history walk.
            let _permit = permit;
            let file = FileBlame::refresh(
                &task_path,
                trust_full,
                previous.as_ref().map(|b| b.file.clone()),
                &task_cancel,
            )?;
            anyhow::ensure!(!task_cancel.is_canceled(), "Blame request canceled");
            if let Some(previous) =
                previous.filter(|b| Arc::ptr_eq(&b.file, &file) && b.text.is_instance(&text))
            {
                return Ok(previous);
            }
            let (base, ..) = helix_view::document::from_reader(&mut file.base(), Some(encoding))?;
            let result = MappedBlame::new(file, base, text)?;
            anyhow::ensure!(!task_cancel.is_canceled(), "Blame request canceled");
            anyhow::Ok(Arc::new(result))
        });
        let Some(result) = cancelable_future(result, &cancel).await else {
            return;
        };
        let result = result
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        job::dispatch(move |editor, _| {
            if cancel.is_canceled() {
                return;
            }
            let Some(doc) = editor.document(doc_id) else {
                return;
            };
            let current_trust = editor
                .workspace_trust
                .query(doc.workspace_root(), TrustQuery::Git)
                .is_trusted();
            let manual_view_matches = line.as_ref().is_none_or(|query| {
                editor
                    .tree
                    .try_get(query.view)
                    .is_some_and(|view| view.doc == doc_id)
            });
            let doc = editor.document_mut(doc_id).unwrap();
            if doc.path() != Some(path.as_path()) || current_trust != trust_full {
                return;
            }
            let error = match result {
                Ok(blame) if blame.text.is_instance(doc.text()) => {
                    doc.file_blame = Some(blame);
                    None
                }
                Ok(_) => return,
                Err(err) => {
                    log::debug!("Failed to obtain blame for {}: {err:#}", path.display());
                    doc.file_blame = None;
                    Some(err.to_string())
                }
            };
            if let Some(BlameQuery {
                view,
                line,
                version,
                ..
            }) = line
            {
                if doc.version() != version
                    || !doc.selections().contains_key(&view)
                    || doc.cursor_line(view) != line as usize
                    || !manual_view_matches
                {
                    return;
                }
                let message = match error {
                    Some(error) => format!("Unable to get blame: {error}"),
                    None => doc
                        .line_blame(line, &doc.config.load().inline_blame.format)
                        .unwrap_or_else(|err| err.to_string()),
                };
                editor.set_status(message);
            }
        })
        .await;
    });
}

pub(super) fn register_hooks(handlers: &Handlers) {
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        if event.editor.document(event.doc).is_some_and(enabled) {
            send_blocking(
                &tx,
                BlameEvent {
                    doc_id: event.doc,
                    line: None,
                },
            );
        }
        Ok(())
    });
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        // Never display blame for an older buffer or let its task overwrite a
        // newer request. Keep the HEAD snapshot to reuse after the debounce.
        event.doc.blame_controller.cancel();
        if !event.ghost_transaction && enabled(event.doc) {
            send_blocking(
                &tx,
                BlameEvent {
                    doc_id: event.doc.id(),
                    line: None,
                },
            );
        }
        Ok(())
    });
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        let trust_changed = event.old.workspace_trust != event.new.workspace_trust;
        for doc in event.editor.documents_mut() {
            if trust_changed
                || (!event.new.inline_blame.auto_fetch
                    && event.new.inline_blame.show == InlineBlameShow::Never)
            {
                doc.clear_blame();
            }
            if event.new.inline_blame.auto_fetch
                && (!event.old.inline_blame.auto_fetch || trust_changed)
            {
                send_blocking(
                    &tx,
                    BlameEvent {
                        doc_id: doc.id(),
                        line: None,
                    },
                );
            }
        }
        Ok(())
    });
    let tx = handlers.blame.clone();
    register_hook!(move |event: &mut PostCommand<'_, '_>| {
        let editor = &mut event.cx.editor;
        let (_, doc) = current_ref!(editor);
        // Refresh HEAD on interaction rather than waking/redrawing an idle
        // editor. This also covers returning to an already open buffer.
        if enabled(doc)
            && !doc.blame_controller.is_running()
            && doc
                .blame_last_check
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(2))
        {
            send_blocking(
                &tx,
                BlameEvent {
                    doc_id: doc.id(),
                    line: None,
                },
            );
        }
        Ok(())
    });
}
