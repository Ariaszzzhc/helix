use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{ensure, Context, Result};
use helix_event::TaskHandle;

use crate::{FileBlame, LineBlame};

use super::{get_repo_dir, open_repo};

pub(crate) fn refresh(
    file: &Path,
    trust_full: bool,
    previous: Option<Arc<FileBlame>>,
    cancel: &TaskHandle,
) -> Result<Arc<FileBlame>> {
    ensure!(!cancel.is_canceled(), "Blame request canceled");
    let file = gix::path::realpath(file).context("resolve symlinks")?;
    let repo = open_repo(get_repo_dir(&file)?, trust_full)?.to_thread_local();
    let head = repo.head_commit()?.id;
    if let Some(previous) = previous.filter(|previous| previous.head == head.to_string()) {
        return Ok(previous);
    }
    let workdir = repo.workdir().context("Git working tree not found")?;
    let relative = gix::path::to_unix_separators_on_windows(gix::path::try_into_bstr(
        file.strip_prefix(workdir)?,
    )?);
    let outcome = repo.blame_file(relative.as_ref(), head, Default::default())?;
    ensure!(!cancel.is_canceled(), "Blame request canceled");
    let mut commits = HashMap::new();
    let mut entries = Vec::with_capacity(outcome.entries.len());
    for entry in outcome.entries {
        ensure!(!cancel.is_canceled(), "Blame request canceled");
        let blame = match commits.entry(entry.commit_id) {
            std::collections::hash_map::Entry::Occupied(entry) => Arc::clone(entry.get()),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let commit = repo.find_commit(*entry.key())?;
                let author = commit.author()?;
                let time = author.time()?;
                let message = commit.message()?;
                let blame = Arc::new(LineBlame {
                    commit_hash: commit.id.to_hex_with_len(8).to_string(),
                    author_name: author.name.to_string(),
                    author_email: author.email.to_string(),
                    commit_date: time.format_or_unix(gix::date::time::format::SHORT),
                    commit_title: message.title.to_string().trim_end().to_owned(),
                    commit_body: message
                        .body
                        .map(|body| body.to_string())
                        .unwrap_or_default(),
                    time_stamp: (time.seconds, time.offset),
                });
                entry.insert(blame).clone()
            }
        };
        entries.push((
            entry.start_in_blamed_file..entry.start_in_blamed_file + entry.len.get(),
            blame,
        ));
    }
    // Use the same trusted filter pipeline and CRLF conversion as the diff
    // gutter; reject a snapshot if HEAD moved while it was being computed.
    let base = super::get_diff_base(&file, trust_full)?;
    ensure!(
        repo.head_commit()?.id == head,
        "HEAD changed while computing blame"
    );
    Ok(Arc::new(FileBlame {
        head: head.to_string(),
        entries,
        base,
    }))
}
