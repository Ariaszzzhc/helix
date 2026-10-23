use std::{collections::HashSet, sync::Arc, time::Duration};

use helix_core::syntax::config::LanguageServerFeature;
use helix_event::{cancelable_future, register_hook, AsyncHook};
use helix_view::{
    breadcrumb,
    editor::CompleteAction,
    events::{
        ConfigDidChange, DocumentDidChange, DocumentDidClose, DocumentDidOpen,
        LanguageServerExited, LanguageServerInitialized,
    },
    handlers::Handlers,
    DocumentId, Editor,
};
use tokio::time::Instant;

use crate::job;

#[derive(Default)]
pub(super) struct Handler {
    docs: HashSet<DocumentId>,
}

impl AsyncHook for Handler {
    type Event = DocumentId;

    fn handle_event(&mut self, doc: DocumentId, _: Option<Instant>) -> Option<Instant> {
        self.docs.insert(doc);
        Some(Instant::now() + Duration::from_millis(250))
    }

    fn finish_debounce(&mut self) {
        let docs = std::mem::take(&mut self.docs);
        job::dispatch_blocking(move |editor, _| {
            for id in docs {
                request(editor, id);
            }
        });
    }
}

fn request(editor: &mut Editor, id: DocumentId) {
    let config = editor.config();
    if !config.breadcrumb.visible() || !config.breadcrumb.symbols {
        return;
    }
    let loader = editor.syn_loader.load();
    let preview = matches!(
        editor.last_completion,
        Some(CompleteAction::Selected { .. })
    ) && editor
        .tree
        .try_get(editor.tree.focus)
        .is_some_and(|view| view.doc == id);
    let Some(doc) = editor.document_mut(id) else {
        return;
    };
    let cancel = doc.breadcrumb_controller.restart();
    let version = doc.version();
    let text = doc.text().clone();
    let fallback = doc
        .syntax()
        .map(|syntax| breadcrumb::from_syntax(syntax, &loader, &text))
        .unwrap_or_default();
    doc.breadcrumb_symbols = Arc::new(fallback);
    doc.breadcrumb_version = Some(version);

    // Use the first supporting server, as for other per-document LSP features.
    // Combining trees from multiple servers creates duplicate or conflicting chains.
    // Completion previews are local ghost edits, absent from the server's text.
    // Rebuild syntax breadcrumbs, but never apply LSP ranges to preview text.
    if !config.lsp.enable || preview {
        return;
    }
    let Some(server) = doc
        .language_servers_with_feature(LanguageServerFeature::DocumentSymbols)
        .next()
    else {
        return;
    };
    let server_id = server.id();
    let encoding = server.offset_encoding();
    let uri = doc.identifier().uri;
    let fallback = doc.breadcrumb_symbols.clone();
    let Some(future) = server.document_symbols(doc.identifier()) else {
        return;
    };
    tokio::spawn(async move {
        let Some(result) = cancelable_future(future, &cancel).await else {
            return;
        };
        let mut response = match result {
            Ok(Some(response)) => response,
            Ok(None) => return,
            Err(err) => {
                log::debug!("breadcrumb symbols request failed: {err}");
                return;
            }
        };
        if let helix_lsp::lsp::DocumentSymbolResponse::Flat(symbols) = &mut response {
            symbols.retain(|symbol| symbol.location.uri == uri);
        }
        let symbols = breadcrumb::from_lsp(response, &text, encoding, &fallback);
        job::dispatch(move |editor, _| {
            let config = editor.config();
            if !config.breadcrumb.visible() || !config.breadcrumb.symbols {
                return;
            }
            let Some(doc) = editor.document_mut(id) else {
                return;
            };
            if !cancel.is_canceled()
                && doc.version() == version
                && doc.supports_language_server(server_id)
            {
                doc.breadcrumb_symbols = Arc::new(symbols);
                doc.breadcrumb_version = Some(version);
            }
        })
        .await;
    });
}

fn refresh(editor: &mut Editor) {
    let ids: Vec<_> = editor.documents().map(|doc| doc.id()).collect();
    for id in ids {
        request(editor, id);
    }
}

pub(super) fn register_hooks(handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        request(event.editor, event.doc);
        Ok(())
    });
    let tx = handlers.breadcrumb.clone();
    register_hook!(move |event: &mut DocumentDidChange<'_>| {
        event.doc.breadcrumb_controller.cancel();
        event.doc.breadcrumb_version = None;
        let config = event.doc.config.load();
        // Aborting a completion also restores its savepoint through a ghost edit.
        // Both the preview and the restoration need a versioned cache refresh.
        if config.breadcrumb.visible() && config.breadcrumb.symbols {
            helix_event::send_blocking(&tx, event.doc.id());
        }
        Ok(())
    });
    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        event.doc.breadcrumb_controller.cancel();
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerInitialized<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(move |event: &mut LanguageServerExited<'_>| {
        refresh(event.editor);
        Ok(())
    });
    register_hook!(move |event: &mut ConfigDidChange<'_>| {
        if event.old.breadcrumb != event.new.breadcrumb
            || event.old.lsp.enable != event.new.lsp.enable
        {
            for doc in event.editor.documents_mut() {
                doc.breadcrumb_controller.cancel();
                doc.breadcrumb_symbols = Arc::default();
                doc.breadcrumb_version = None;
            }
            refresh(event.editor);
        } else {
            // A config reload can replace language definitions and syntax trees
            // even when the breadcrumb settings themselves have not changed.
            let ids: Vec<_> = event
                .editor
                .documents()
                .filter(|doc| doc.breadcrumb_version.is_none())
                .map(|doc| doc.id())
                .collect();
            for id in ids {
                request(event.editor, id);
            }
        }
        Ok(())
    });
}
