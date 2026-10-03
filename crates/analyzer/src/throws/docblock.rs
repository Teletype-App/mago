use mago_allocator::Arena;
use mago_codex::metadata::function_like::FunctionLikeMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::comparator::ComparisonResult;
use mago_codex::ttype::comparator::union_comparator;
use mago_codex::ttype::union::TUnion;
use mago_phpdoc_syntax::PHPDocParser;
use mago_phpdoc_syntax::cst::TagValue;
use mago_reporting::{Annotation, Issue};
use mago_span::{HasSpan, Span};
use mago_syntax::comments::docblock::PrecedingDocblocks;
use mago_text_edit::{Safety, TextEdit};
use mago_word::Word;

use crate::code::IssueCode;
use crate::context::Context;
use crate::context::block::BlockContext;

pub(crate) struct DocblockChanges {
    pub edits: Vec<TextEdit>,
    pub issues: Vec<(IssueCode, Issue)>,
}

fn covered(context: &Context<'_, '_, impl Arena>, value: &TUnion, declared: &TUnion) -> bool {
    union_comparator::is_contained_by(
        context.codebase,
        value,
        declared,
        false,
        false,
        false,
        &mut ComparisonResult::default(),
    )
}

fn union(exception: Word) -> TUnion {
    TUnion::from_atomic(TAtomic::Object(TObject::new_named(exception)))
}

fn render(context: &Context<'_, '_, impl Arena>, exception: Word) -> String {
    let name = context.codebase.get_class_like(exception.as_bytes()).map_or(exception, |class| class.original_name);
    format!("\\{name}")
}

pub(crate) fn prepare<A>(
    context: &Context<'_, '_, A>,
    block: &BlockContext<'_>,
    metadata: &FunctionLikeMetadata,
    actual: &[Word],
    expected: &[(Span, TUnion)],
) -> DocblockChanges
where
    A: Arena,
{
    let complete = block.unresolved_throw_calls.is_empty();
    let missing = actual
        .iter()
        .copied()
        .filter(|exception| {
            !crate::statement::function_like::is_exception_unchecked(context, *exception)
                && !expected.iter().any(|(_, declared)| covered(context, &union(*exception), declared))
        })
        .collect::<Vec<_>>();
    let mut changes = DocblockChanges { edits: Vec::new(), issues: Vec::new() };
    let mut replacements = Vec::<(Span, String)>::new();
    if complete {
        for declared in &metadata.thrown_types {
            let Some((_, expanded)) = expected.iter().find(|(span, _)| *span == declared.span) else {
                continue;
            };
            let matching = actual
                .iter()
                .copied()
                .filter(|exception| covered(context, &union(*exception), expanded))
                .collect::<Vec<_>>();
            if matching
                .iter()
                .any(|exception| crate::statement::function_like::is_exception_unchecked(context, *exception))
            {
                continue;
            }
            if expanded
                .types
                .iter()
                .flat_map(TAtomic::get_all_object_names)
                .any(|exception| crate::statement::function_like::is_exception_unchecked(context, exception))
            {
                continue;
            }
            let exact = matching.iter().fold(None, |acc, exception| {
                Some(match acc {
                    None => union(*exception),
                    Some(previous) => mago_codex::ttype::combine_union_types(
                        &previous,
                        &union(*exception),
                        context.codebase,
                        context.settings.combiner_options(),
                    ),
                })
            });
            let code = if matching.is_empty() {
                IssueCode::UnusedThrowsType
            } else if exact.as_ref().is_some_and(|inferred| !covered(context, expanded, inferred)) {
                IssueCode::OverlyWideThrowsType
            } else {
                continue;
            };
            let inferred = matching.iter().map(|exception| render(context, *exception)).collect::<Vec<_>>().join("|");
            replacements.push((declared.span, inferred.clone()));
            let message = if matching.is_empty() {
                "This exception does not escape the body".to_string()
            } else {
                format!("The body only throws {inferred}")
            };
            changes.issues.push((
                code,
                Issue::warning(message.clone())
                    .with_annotation(Annotation::primary(declared.span).with_message(message))
                    .with_annotation(
                        Annotation::secondary(metadata.name_span.unwrap_or(metadata.span))
                            .with_message("Exception contract inferred from this body"),
                    ),
            ));
        }
    }
    if !complete && !metadata.thrown_types.is_empty() {
        let mut issue = Issue::warning("Exception inference is incomplete. Existing @throws tags will be preserved.")
            .with_annotation(Annotation::primary(metadata.name_span.unwrap_or(metadata.span)))
            .with_note("A call target could not be resolved from the available source and types.");
        for span in &block.unresolved_throw_calls {
            issue = issue.with_annotation(Annotation::secondary(*span).with_message("Unresolved exception effect"));
        }
        changes.issues.push((IssueCode::ThrowsInferenceIncomplete, issue));
    }
    if missing.is_empty() && replacements.is_empty() {
        return changes;
    }

    let mut documents = PrecedingDocblocks::new(context.comments, metadata.span.start.offset).collect::<Vec<_>>();
    documents.reverse();
    let mut added = false;
    for (index, comment) in documents.iter().enumerate() {
        let document = PHPDocParser::parse_with_span(context.arena, comment.value, comment.span);
        if document.has_errors() {
            return DocblockChanges { edits: Vec::new(), issues: changes.issues };
        }
        let mut edits = Vec::<(std::ops::Range<usize>, Vec<u8>)>::new();
        for tag in document.tags() {
            let TagValue::Throws(thrown) = &tag.value else {
                continue;
            };
            let Some((_, replacement)) = replacements.iter().find(|(span, _)| *span == thrown.r#type.span()) else {
                continue;
            };
            let base = comment.span.start.offset;
            if replacement.is_empty() {
                // Retain descriptions when removing an obsolete exception contract.
                let end = thrown
                    .description
                    .as_ref()
                    .map_or(tag.span().end.offset, |description| description.span.start.offset);
                let start = (tag.at.start.offset - base) as usize;
                let end = (end - base) as usize;
                let line_start =
                    comment.value[..start].iter().rposition(|byte| *byte == b'\n').map_or(0, |offset| offset + 1);
                let line_end =
                    comment.value[end..].iter().position(|byte| *byte == b'\n').map(|offset| end + offset + 1);
                let only_tag = thrown.description.is_none()
                    && comment.value[line_start..start].iter().all(|byte| byte.is_ascii_whitespace() || *byte == b'*')
                    && line_end.is_some_and(|offset| comment.value[end..offset].iter().all(u8::is_ascii_whitespace));
                edits.push((if only_tag { line_start..line_end.unwrap_or(end) } else { start..end }, Vec::new()));
            } else {
                edits.push((
                    (thrown.r#type.span().start.offset - base) as usize
                        ..(thrown.r#type.span().end.offset - base) as usize,
                    replacement.as_bytes().to_vec(),
                ));
            }
        }
        let mut value = comment.value.to_vec();
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        for (range, replacement) in edits {
            value.splice(range, replacement);
        }
        if index + 1 == documents.len() && !missing.is_empty() {
            let tags =
                missing.iter().map(|exception| format!("@throws {}", render(context, *exception))).collect::<Vec<_>>();
            append_tags(&mut value, &tags, indentation(context, metadata.span.start.offset));
            added = true;
        }
        if value != comment.value {
            if value.iter().all(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'*')) {
                value.clear();
            }
            changes
                .edits
                .push(TextEdit::replace(comment.span.to_range(), value).with_safety(Safety::PotentiallyUnsafe));
        }
    }
    if !missing.is_empty() && !added {
        let indent = indentation(context, metadata.span.start.offset);
        let newline = newline(context.source_file.contents.as_ref());
        let mut tags = String::new();
        for exception in &missing {
            tags.push_str(indent);
            tags.push_str(" * @throws ");
            tags.push_str(&render(context, *exception));
            tags.push_str(newline);
        }
        let text = format!("/**{newline}{tags}{indent} */{newline}{indent}");
        changes.edits.push(TextEdit::insert(metadata.span.start.offset, text).with_safety(Safety::PotentiallyUnsafe));
    }
    changes
}

fn indentation<'context>(context: &'context Context<'_, '_, impl Arena>, offset: u32) -> &'context str {
    let prefix = &context.source_file.contents[..offset as usize];
    let line = prefix.iter().rposition(|byte| *byte == b'\n').map_or(0, |offset| offset + 1);
    let bytes = &prefix[line..];
    if bytes.iter().all(|byte| matches!(byte, b' ' | b'\t')) { std::str::from_utf8(bytes).unwrap_or("") } else { "" }
}

fn newline(value: &[u8]) -> &'static str {
    if value.windows(2).any(|pair| pair == b"\r\n") { "\r\n" } else { "\n" }
}

fn append_tags(value: &mut Vec<u8>, tags: &[String], indent: &str) {
    let Some(close) = value.windows(2).rposition(|pair| pair == b"*/") else {
        return;
    };
    let newline = newline(value);
    let mut text = String::new();
    for tag in tags {
        text.push_str(indent);
        text.push_str(" * ");
        text.push_str(tag);
        text.push_str(newline);
    }
    if value[..close].contains(&b'\n') {
        let line = value[..close].iter().rposition(|byte| *byte == b'\n').map_or(close, |offset| offset + 1);
        value.splice(line..line, text.bytes());
    } else {
        let inner = String::from_utf8_lossy(&value[3..close]);
        let inner = inner.trim();
        let existing = if inner.is_empty() { String::new() } else { format!("{indent} * {inner}{newline}") };
        *value = format!("/**{newline}{existing}{text}{indent} */").into_bytes();
    }
}
