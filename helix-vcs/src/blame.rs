use std::{ops::Range, path::Path, sync::Arc};

use anyhow::Result;
use helix_core::Rope;
use helix_event::TaskHandle;
use imara_diff::{Algorithm, Hunk};

/// Immutable blame snapshot, with repository access and commit decoding
/// completed on a blocking worker before it is handed to the editor.
#[derive(Debug)]
pub struct FileBlame {
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    pub(crate) head: String,
    pub(crate) entries: Vec<(Range<u32>, Arc<LineBlame>)>,
    pub(crate) base: Vec<u8>,
}

/// Blame mapped to an exact editor buffer, including unsaved changes.
#[derive(Debug)]
pub struct MappedBlame {
    pub file: Arc<FileBlame>,
    pub text: Rope,
    hunks: Vec<Hunk>,
}

impl MappedBlame {
    pub fn new(file: Arc<FileBlame>, base: Rope, text: Rope) -> Result<Self> {
        let input = crate::diff::line_cache::InternedRopeLines::new(base, text.clone());
        let input = input
            .interned_lines()
            .ok_or_else(|| anyhow::anyhow!("File too large for blame"))?;
        let diff = imara_diff::Diff::compute(Algorithm::Histogram, input);
        Ok(Self {
            file,
            text,
            hunks: diff.hunks().collect(),
        })
    }

    /// Added or replaced lines have no committed blame. A deletion only shifts
    /// subsequent line numbers, including the line immediately after the hunk.
    pub fn line(&self, line: u32) -> Option<&LineBlame> {
        let idx = self.hunks.partition_point(|hunk| hunk.after.end <= line);
        if self
            .hunks
            .get(idx)
            .is_some_and(|hunk| hunk.after.contains(&line))
        {
            return None;
        }
        let base_line = if idx == 0 {
            line
        } else {
            let hunk = &self.hunks[idx - 1];
            line.checked_sub(hunk.after.end)?
                .checked_add(hunk.before.end)?
        };
        self.file.line(base_line)
    }
}

impl FileBlame {
    /// Reuse a snapshot if HEAD has not changed. Call only on a blocking worker.
    pub fn refresh(
        file: &Path,
        trust_full: bool,
        previous: Option<Arc<Self>>,
        cancel: &TaskHandle,
    ) -> Result<Arc<Self>> {
        #[cfg(feature = "git")]
        return crate::git::blame::refresh(file, trust_full, previous, cancel);
        #[cfg(not(feature = "git"))]
        {
            let _ = (file, trust_full, previous, cancel);
            anyhow::bail!("Git support is not compiled in")
        }
    }

    pub fn base(&self) -> &[u8] {
        &self.base
    }

    pub fn line(&self, line: u32) -> Option<&LineBlame> {
        let idx = self.entries.partition_point(|(range, _)| range.end <= line);
        self.entries
            .get(idx)
            .filter(|(range, _)| range.contains(&line))
            .map(|(_, blame)| blame.as_ref())
    }
}

#[derive(Debug, Default)]
pub struct LineBlame {
    pub(crate) commit_hash: String,
    pub(crate) author_name: String,
    pub(crate) author_email: String,
    pub(crate) commit_date: String,
    pub(crate) commit_title: String,
    pub(crate) commit_body: String,
    pub(crate) time_stamp: (i64, i32),
}

impl LineBlame {
    /// Expand the format from upstream #13133. Unknown or unfinished
    /// placeholders are preserved, and values cannot introduce virtual lines.
    pub fn format(&self, format: &str) -> String {
        let relative_time = helix_stdx::time::format_relative_time(self.time_stamp.0);
        let mut result = String::with_capacity(format.len());
        let mut remaining = format;
        while let Some(start) = remaining.find('{') {
            result.push_str(&remaining[..start]);
            let Some(end) = remaining[start..].find('}').map(|end| start + end) else {
                result.push_str(&remaining[start..]);
                remaining = "";
                break;
            };
            let value = match &remaining[start + 1..end] {
                "author" => &self.author_name,
                "email" => &self.author_email,
                "date" => &self.commit_date,
                "title" => &self.commit_title,
                "body" => &self.commit_body,
                "commit" => &self.commit_hash,
                "time-ago" => &relative_time,
                _ => &remaining[start..=end],
            };
            result.push_str(value);
            remaining = &remaining[end + 1..];
        }
        result.push_str(remaining);
        result
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_unsaved_insertions_deletions_and_replacements() {
        let base = "alpha\nbeta\ngamma\ndelta\n";
        let file = Arc::new(FileBlame {
            head: String::new(),
            base: base.as_bytes().to_vec(),
            entries: (0..4)
                .map(|line| {
                    (
                        line..line + 1,
                        Arc::new(LineBlame {
                            commit_title: line.to_string(),
                            ..LineBlame::default()
                        }),
                    )
                })
                .collect(),
        });
        for (text, expected) in [
            (base, vec![Some("0"), Some("1"), Some("2"), Some("3")]),
            (
                "new\nalpha\nbeta\ngamma\ndelta\n",
                vec![None, Some("0"), Some("1"), Some("2"), Some("3")],
            ),
            (
                "alpha\ngamma\ndelta\n",
                vec![Some("0"), Some("2"), Some("3")],
            ),
            (
                "alpha\nchanged\ngamma\ndelta\n",
                vec![Some("0"), None, Some("2"), Some("3")],
            ),
            (
                "alpha\nbeta\ngamma\ndelta\nnew\n",
                vec![Some("0"), Some("1"), Some("2"), Some("3"), None],
            ),
            ("", vec![None]),
        ] {
            let mapped =
                MappedBlame::new(file.clone(), Rope::from_str(base), Rope::from_str(text)).unwrap();
            for (line, expected) in expected.into_iter().enumerate() {
                assert_eq!(
                    mapped
                        .line(line as u32)
                        .map(|blame| blame.commit_title.as_str()),
                    expected,
                    "{text:?}: line {line}"
                );
            }
        }
    }

    #[test]
    fn format_preserves_literals_and_unknown_variables() {
        let blame = LineBlame {
            author_name: "作者".into(),
            commit_title: "Fix\nissue".into(),
            ..LineBlame::default()
        };
        for (format, expected) in [
            ("By {author}: {title}.", "By 作者: Fix issue."),
            ("literal", "literal"),
            (
                "{unknown} {author} {unfinished",
                "{unknown} 作者 {unfinished",
            ),
            ("{body}", ""),
        ] {
            assert_eq!(blame.format(format), expected);
        }
    }
}
