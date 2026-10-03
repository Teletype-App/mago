use foldhash::{HashMap, HashSet};
use mago_allocator::Arena;
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::ttype::union::TUnion;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::{Expression, Variable};
use mago_word::{Word, WordMap, word};

use super::{FunctionThrowsSummary, ThrowCondition};
use crate::artifacts::AnalysisArtifacts;
use crate::context::{Context, block::BlockContext};
use crate::invocation::Invocation;

pub(crate) fn effects<'a>(
    expression: &Expression<'_>,
    block: &'a BlockContext<'_>,
    artifacts: &'a AnalysisArtifacts,
) -> Option<&'a FunctionThrowsSummary> {
    if let Expression::Variable(Variable::Direct(variable)) = expression {
        return block.generator_throws.get(&word(variable.name));
    }
    if let Expression::Parenthesized(parenthesized) = expression {
        return effects(parenthesized.expression, block, artifacts);
    }
    let span = expression.span();
    artifacts.generator_throws.get(&(span.start.offset, span.end.offset))
}

pub(crate) fn merge(destination: &mut FunctionThrowsSummary, source: &FunctionThrowsSummary) {
    for (exception, conditions) in &source.exceptions {
        for condition in conditions {
            destination.add(*exception, condition.clone());
        }
    }
    for (exception, origins) in &source.provenance {
        let retained = destination.provenance.entry(*exception).or_default();
        retained.extend(origins.iter().cloned());
        retained.sort();
        retained.dedup();
    }
    destination.unresolved_calls.extend(source.unresolved_calls.iter().copied());
}

pub(crate) fn consume<A: Arena>(
    context: &Context<'_, '_, A>,
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    expression: &Expression<'_>,
) {
    if !context.settings.throws_enabled() {
        return;
    }
    let deferred = effects(expression, block, artifacts).cloned();
    let unknown_generator = artifacts
        .get_expression_type(expression)
        .is_some_and(|ty| ty.types.iter().any(|atomic| atomic.extends_or_implements(context.codebase, b"Generator")));
    apply(block, artifacts, expression.span(), deferred, unknown_generator);
}

pub(crate) fn consume_method(
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    variable: Option<&[u8]>,
    span: Span,
) {
    let deferred = variable.and_then(|variable| block.generator_throws.get(&word(variable))).cloned();
    apply(block, artifacts, span, deferred, true);
}

fn apply(
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    span: Span,
    deferred: Option<FunctionThrowsSummary>,
    unknown_generator: bool,
) {
    if let Some(deferred) = deferred {
        for exception in deferred.exceptions.keys() {
            block.possibly_thrown_exceptions.entry(*exception).or_default().insert(span);
            super::record_throw(artifacts, block, *exception, span);
            for target in deferred.provenance.get(exception).into_iter().flatten().filter_map(|origin| origin.target) {
                artifacts.throw_targets.entry((*exception, span)).or_default().insert(target);
            }
        }
        if !deferred.unresolved_calls.is_empty() {
            block.unresolved_throw_calls.insert(span);
        }
    } else if unknown_generator {
        block.unresolved_throw_calls.insert(span);
    }
}

pub(crate) struct SavedEffects {
    exceptions: WordMap<HashSet<Span>>,
    unresolved: HashSet<Span>,
    conditions: HashMap<(Word, Span), Vec<ThrowCondition>>,
    targets: HashMap<(Word, Span), std::collections::BTreeSet<FunctionLikeIdentifier>>,
}

impl SavedEffects {
    pub(crate) fn take(block: &mut BlockContext<'_>, artifacts: &mut AnalysisArtifacts) -> Self {
        Self {
            exceptions: std::mem::take(&mut block.possibly_thrown_exceptions),
            unresolved: std::mem::take(&mut block.unresolved_throw_calls),
            conditions: std::mem::take(&mut artifacts.throw_conditions),
            targets: std::mem::take(&mut artifacts.throw_targets),
        }
    }

    pub(crate) fn finish(self, block: &mut BlockContext<'_>, artifacts: &mut AnalysisArtifacts, span: Span) {
        let mut effects = FunctionThrowsSummary::collect(block, artifacts);
        effects.returned_generator = None;
        block.possibly_thrown_exceptions = self.exceptions;
        block.unresolved_throw_calls = self.unresolved;
        artifacts.throw_conditions = self.conditions;
        artifacts.throw_targets = self.targets;
        artifacts.generator_throws.insert((span.start.offset, span.end.offset), effects);
    }
}

pub(crate) fn returned<A: Arena>(
    context: &Context<'_, '_, A>,
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'_, '_, '_>,
    summary: &FunctionThrowsSummary,
    parameters: &WordMap<TUnion>,
    target: FunctionLikeIdentifier,
) {
    let saved = SavedEffects::take(block, artifacts);
    super::propagate(context, block, artifacts, invocation, summary, parameters, target);
    block.unresolved_throw_calls.extend(summary.unresolved_calls.iter().copied());
    saved.finish(block, artifacts, invocation.span);
}
