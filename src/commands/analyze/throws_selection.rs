use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Command;

use foldhash::HashMap;
use mago_allocator::LocalArena;
use mago_database::file::{FileId, FileType};
use mago_database::{DatabaseReader, ReadDatabase};
use mago_reporting::{Issue, IssueCollection};
use mago_span::{HasSpan, Span};
use mago_syntax::comments::docblock::PrecedingDocblocks;
use mago_syntax::cst::{ArrowFunction, Closure, Function, Method, Trivia};
use mago_syntax::parser::parse_file_with_settings;
use mago_syntax::settings::ParserSettings;
use mago_syntax::walker::MutWalker;

use crate::error::Error;

pub(super) struct ThrowsSelection {
    regions: HashMap<FileId, Vec<RangeInclusive<u32>>>,
}

fn git(workspace: &Path, arguments: &[&str]) -> Result<std::process::Output, Error> {
    Command::new("git")
        .args(arguments)
        .current_dir(workspace)
        .output()
        .map_err(|error| Error::InvalidArgument(format!("Cannot read Git diff: {error}")))
}

impl ThrowsSelection {
    pub fn load(
        workspace: &Path,
        reference: &str,
        full_files: &[PathBuf],
        database: &ReadDatabase,
        settings: ParserSettings,
    ) -> Result<Self, Error> {
        let root = git(workspace, &["rev-parse", "--show-toplevel"])?;
        if !root.status.success() {
            return Err(Error::NotAGitRepository);
        }
        let root = PathBuf::from(String::from_utf8_lossy(&root.stdout).trim());
        let revision =
            git(workspace, &["rev-parse", "--verify", "--end-of-options", &format!("{reference}^{{commit}}")])?;
        if !revision.status.success() {
            return Err(Error::InvalidArgument(format!("Cannot resolve Git base {reference}")));
        }
        let revision = String::from_utf8_lossy(&revision.stdout).trim().to_string();
        let full_files = full_files
            .iter()
            .map(|path| if path.is_absolute() { path.clone() } else { workspace.join(path) })
            .collect::<Vec<_>>();
        let mut regions = HashMap::default();
        for file in database.files().filter(|file| file.file_type == FileType::Host) {
            let Some(path) = file.path.as_ref() else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            if full_files.iter().any(|selected| selected == path) {
                regions.insert(file.id, vec![0..=file.size]);
                continue;
            }
            let relative = relative.to_string_lossy();
            let exists = git(workspace, &["cat-file", "-e", &format!("{revision}:{relative}")])?;
            if !exists.status.success() {
                regions.insert(file.id, vec![0..=file.size]);
                continue;
            }
            let diff = git(
                workspace,
                &["diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--unified=0", &revision, "--", &relative],
            )?;
            if !diff.status.success() {
                return Err(Error::InvalidArgument(format!("Cannot read diff for {relative}")));
            }
            let lines = changed_lines(&diff.stdout);
            if lines.is_empty() {
                continue;
            }
            let arena = LocalArena::new();
            let program = parse_file_with_settings(&arena, &file, settings);
            let mut collector = DeclarationCollector { comments: program.trivia.as_slice(), spans: Vec::new() };
            collector.walk_program(program, &mut ());
            let mut selected = collector
                .spans
                .iter()
                .filter(|span| {
                    lines.iter().any(|range| {
                        range.start() <= &(file.line_number(span.end.offset) + 1)
                            && range.end() >= &(file.line_number(span.start.offset) + 1)
                    })
                })
                .map(|span| span.start.offset..=span.end.offset)
                .collect::<Vec<_>>();
            // Keep changed global statements visible as well.
            for line in lines {
                let start = file.get_line_start_offset(line.start().saturating_sub(1)).unwrap_or(file.size);
                let end = file.get_line_end_offset(line.end().saturating_sub(1)).unwrap_or(file.size);
                selected.push(start..=end);
            }
            regions.insert(file.id, selected);
        }
        Ok(Self { regions })
    }

    pub fn filter(&self, issues: IssueCollection) -> IssueCollection {
        IssueCollection::from(issues.into_iter().filter_map(|mut issue| {
            if !self.contains_issue(&issue) {
                return None;
            }
            issue.edits.retain(|file, edits| {
                edits.retain(|edit| {
                    self.regions.get(file).is_some_and(|regions| {
                        regions.iter().any(|range| range.contains(&edit.range.start) && range.contains(&edit.range.end))
                    })
                });
                !edits.is_empty()
            });
            Some(issue)
        }))
    }

    fn contains_issue(&self, issue: &Issue) -> bool {
        if issue.code.as_deref().is_some_and(|code| code == "parse" || code == "syntax") {
            return true;
        }
        issue.annotations.iter().filter(|annotation| annotation.kind.is_primary()).any(|annotation| {
            self.regions
                .get(&annotation.span.file_id)
                .is_some_and(|regions| regions.iter().any(|range| range.contains(&annotation.span.start.offset)))
        })
    }
}

fn changed_lines(diff: &[u8]) -> Vec<RangeInclusive<u32>> {
    diff.split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let header = std::str::from_utf8(line).ok()?.strip_prefix("@@ ")?;
            let new = header.split_whitespace().nth(1)?.strip_prefix('+')?;
            let (start, count) = new.split_once(',').unwrap_or((new, "1"));
            let start = start.parse::<u32>().ok()?.max(1);
            let count = count.parse::<u32>().ok()?;
            Some(start..=start.saturating_add(count.saturating_sub(1)))
        })
        .collect()
}

struct DeclarationCollector<'arena> {
    comments: &'arena [Trivia<'arena>],
    spans: Vec<Span>,
}

impl DeclarationCollector<'_> {
    fn collect(&mut self, mut span: Span) {
        if let Some(comment) = PrecedingDocblocks::new(self.comments, span.start.offset).last() {
            span.start = comment.span.start;
        }
        self.spans.push(span);
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for DeclarationCollector<'arena> {
    fn walk_in_function(&mut self, node: &'ast Function<'arena>, _: &mut ()) {
        self.collect(node.span());
    }
    fn walk_in_method(&mut self, node: &'ast Method<'arena>, _: &mut ()) {
        self.collect(node.span());
    }
    fn walk_in_closure(&mut self, node: &'ast Closure<'arena>, _: &mut ()) {
        self.collect(node.span());
    }
    fn walk_in_arrow_function(&mut self, node: &'ast ArrowFunction<'arena>, _: &mut ()) {
        self.collect(node.span());
    }
}
