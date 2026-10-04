use mago_allocator::Arena;
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::ttype::atomic::callable::TCallable;
use mago_codex::ttype::cast::cast_atomic_to_callable;
use mago_codex::ttype::union::TUnion;
use mago_word::WordMap;

use crate::artifacts::AnalysisArtifacts;
use crate::context::{Context, block::BlockContext};
use crate::invocation::Invocation;

pub(crate) fn collect<A>(
    context: &Context<'_, '_, A>,
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'_, '_, '_>,
    parameters: &WordMap<TUnion>,
) where
    A: Arena,
{
    let Some(summaries) = context.throws_summaries else {
        return;
    };
    let Some(metadata) = invocation.target.get_function_like_metadata() else {
        return;
    };
    if summaries.source_files.contains(&metadata.span.file_id) {
        return;
    }
    let native_callback = match invocation.target.get_function_like_identifier() {
        Some(FunctionLikeIdentifier::Function(name)) => match name.as_bytes() {
            b"array_map" => Some(0),
            b"array_all"
            | b"array_any"
            | b"array_filter"
            | b"array_find"
            | b"array_find_key"
            | b"array_reduce"
            | b"array_walk"
            | b"array_walk_recursive"
            | b"iterator_apply"
            | b"preg_replace_callback"
            | b"uasort"
            | b"uksort"
            | b"usort" => Some(1),
            _ => None,
        },
        _ => None,
    };
    if native_callback.is_some()
        && let Some(FunctionLikeIdentifier::Function(name)) = invocation.target.get_function_like_identifier()
        && name.as_bytes().starts_with(b"array_")
    {
        let array_index = usize::from(name.as_bytes() == b"array_map");
        if metadata
            .parameters
            .get(array_index)
            .and_then(|parameter| parameters.get(&parameter.name.0))
            .is_some_and(|value| value.is_empty_array())
            && (name.as_bytes() != b"array_map"
                || invocation.arguments_source.iter_arguments().enumerate().all(|(index, argument)| {
                    if argument.is_unpacked() {
                        return false;
                    }
                    let callback = argument.get_parameter_name().map_or(index == 0, |name| name == b"callback");
                    callback
                        || argument
                            .value()
                            .and_then(|expression| artifacts.get_expression_type(expression))
                            .is_some_and(|value| value.is_empty_array())
                }))
        {
            return;
        }
    }
    for (index, parameter) in metadata.parameters.iter().enumerate() {
        if !parameter.immediately_invoked_callable && native_callback != Some(index) {
            continue;
        }
        let Some(value) = parameters.get(&parameter.name.0) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        for atomic in value.types.iter() {
            let target =
                cast_atomic_to_callable(atomic, context.codebase, None).and_then(|callable| match callable.as_ref() {
                    TCallable::Alias(target) => Some(*target),
                    TCallable::Signature(signature) => signature.source,
                });
            let Some(target) = target else {
                block.unresolved_throw_calls.insert(invocation.span);
                continue;
            };
            let target = super::canonical(match target {
                FunctionLikeIdentifier::Method(class, method) => context
                    .codebase
                    .get_declaring_method_identifier(&mago_codex::identifier::method::MethodIdentifier::new(
                        class, method,
                    ))
                    .into(),
                _ => target,
            });
            if let Some(metadata) = context.codebase.get_function_like(&target) {
                artifacts.throws_dependencies.insert(metadata.span.file_id);
            }
            let summary = artifacts.inferred_throws.get(&target).or_else(|| summaries.functions.get(&target)).cloned();
            if context.throws_inference {
                artifacts.throws_summary_reads.insert(super::SummaryKey::Function(target));
            }
            if let Some(summary) = summary {
                for exception in summary.exceptions.keys() {
                    block.possibly_thrown_exceptions.entry(*exception).or_default().insert(invocation.span);
                    artifacts.throw_targets.entry((*exception, invocation.span)).or_default().insert(target);
                    super::record_throw(artifacts, block, *exception, invocation.span);
                }
                if !summary.unresolved_calls.is_empty() {
                    block.unresolved_throw_calls.insert(invocation.span);
                }
            } else if let Some(metadata) = context.codebase.get_function_like(&target)
                && (!summaries.source_files.contains(&metadata.span.file_id)
                    || metadata.method_metadata.as_ref().is_some_and(|method| method.is_abstract))
            {
                for declared in &metadata.thrown_types {
                    for exception in declared.type_union.types.iter().flat_map(|atomic| atomic.get_all_object_names()) {
                        block.possibly_thrown_exceptions.entry(exception).or_default().insert(invocation.span);
                        artifacts.throw_targets.entry((exception, invocation.span)).or_default().insert(target);
                        super::record_throw(artifacts, block, exception, invocation.span);
                    }
                }
            } else if context.codebase.get_function_like(&target).is_none() {
                block.unresolved_throw_calls.insert(invocation.span);
            }
        }
    }
}
