pub(crate) mod callbacks;
pub(crate) mod docblock;
pub(crate) mod generator;
pub(crate) mod native;
pub mod yii2;

pub(crate) fn canonical(identifier: FunctionLikeIdentifier) -> FunctionLikeIdentifier {
    use mago_word::ascii_lowercase_word;
    match identifier {
        FunctionLikeIdentifier::Function(name) => {
            FunctionLikeIdentifier::Function(ascii_lowercase_word(name.as_bytes()))
        }
        FunctionLikeIdentifier::Method(class, method) => FunctionLikeIdentifier::Method(
            ascii_lowercase_word(class.as_bytes()),
            ascii_lowercase_word(method.as_bytes()),
        ),
        FunctionLikeIdentifier::Closure(_) => identifier,
    }
}

pub(crate) fn function_metadata<'metadata>(
    codebase: &'metadata mago_codex::metadata::CodebaseMetadata,
    identifier: &FunctionLikeIdentifier,
) -> Option<&'metadata mago_codex::metadata::function_like::FunctionLikeMetadata> {
    codebase.get_function_like(identifier).or_else(|| match identifier {
        FunctionLikeIdentifier::Method(class, method) => {
            codebase.get_declaring_method(class.as_bytes(), method.as_bytes())
        }
        _ => None,
    })
}

use crate::artifacts::AnalysisArtifacts;
use crate::context::Context;
use crate::context::block::BlockContext;
use crate::invocation::Invocation;
use foldhash::HashMap;
use foldhash::HashSet;
use mago_allocator::Arena;
use mago_allocator::LocalArena;
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::reference::SymbolReferences;
use mago_codex::ttype::comparator::union_comparator::can_expression_types_be_identical;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{get_false, get_literal_int, get_literal_string, get_null, get_true};
use mago_database::file::File;
use mago_database::file::FileId;
use mago_names::resolver::NameResolver;
use mago_span::Span;
use mago_syntax::cst::{Expression, Variable};
use mago_syntax::parser::parse_file_with_settings;
use mago_syntax::settings::ParserSettings;
use mago_word::Word;
use mago_word::WordMap;
use mago_word::word;
use rayon::prelude::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConditionValue {
    Bool(bool),
    Int(i64),
    String(Word),
    Null,
}

impl ConditionValue {
    fn from_type(value: &TUnion) -> Option<Self> {
        if value.is_true() {
            Some(Self::Bool(true))
        } else if value.is_false() {
            Some(Self::Bool(false))
        } else if value.is_null() {
            Some(Self::Null)
        } else if let Some(value) = value.get_single_literal_int_value() {
            Some(Self::Int(value))
        } else {
            value.get_single_literal_string_value().map(|value| Self::String(word(value)))
        }
    }

    fn union(&self) -> TUnion {
        match self {
            Self::Bool(true) => get_true(),
            Self::Bool(false) => get_false(),
            Self::Int(value) => get_literal_int(*value),
            Self::String(value) => get_literal_string(*value),
            Self::Null => get_null(),
        }
    }
}

pub type ThrowCondition = BTreeMap<usize, ConditionValue>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FunctionThrowsSummary {
    pub returned_generator: Option<Box<FunctionThrowsSummary>>,
    #[cfg_attr(feature = "serde", serde(with = "word_map_serde"))]
    pub provenance: WordMap<Vec<ThrowSite>>,
    #[cfg_attr(feature = "serde", serde(with = "word_map_serde"))]
    pub exceptions: WordMap<Vec<ThrowCondition>>,
    pub unresolved_calls: HashSet<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ThrowSite {
    pub span: Span,
    pub target: Option<FunctionLikeIdentifier>,
}

#[cfg(feature = "serde")]
pub(super) mod word_map_serde {
    use mago_word::{Word, WordMap};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S, T>(map: &WordMap<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        let mut pairs = map.iter().collect::<Vec<_>>();
        pairs.sort_by_key(|(key, _)| **key);
        pairs.serialize(serializer)
    }

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<WordMap<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        Vec::<(Word, T)>::deserialize(deserializer).map(|pairs| pairs.into_iter().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ThrowsContext {
    pub function: FunctionLikeIdentifier,
    pub arguments: Vec<(usize, TUnion)>,
}

pub(crate) fn invocation_context<A>(
    context: &Context<'_, '_, A>,
    invocation: &Invocation<'_, '_, '_>,
    parameters: &WordMap<TUnion>,
) -> Option<ThrowsContext>
where
    A: Arena,
{
    let metadata = invocation.target.get_function_like_metadata()?;
    if context.throws_summaries.is_some_and(|summaries| !summaries.source_files.contains(&metadata.span.file_id)) {
        return None;
    }
    let mut function = invocation
        .target
        .get_method_context()
        .and_then(|method| method.declaring_method_id)
        .map(FunctionLikeIdentifier::from)
        .or_else(|| invocation.target.get_function_like_identifier().copied())?;
    let trait_receiver = invocation.target.get_method_context().is_some_and(|method| {
        let Some(declaring) = method.declaring_method_id else {
            return false;
        };
        context
            .codebase
            .get_class_like(declaring.get_class_name().as_bytes())
            .is_some_and(|class| class.kind.is_trait())
            && method.class_like_metadata.name != declaring.get_class_name()
    });
    if trait_receiver && let Some(method) = invocation.target.get_method_context() {
        function = FunctionLikeIdentifier::Method(method.class_like_metadata.name, metadata.name);
    }
    let arguments = metadata
        .parameters
        .iter()
        .enumerate()
        .filter_map(|(index, parameter)| {
            let value = parameters.get(&parameter.name.0)?;
            let concrete_callable = value.types.iter().any(|atomic| {
                mago_codex::ttype::cast::cast_atomic_to_callable(atomic, context.codebase, None).is_some_and(
                    |callable| match callable.as_ref() {
                        mago_codex::ttype::atomic::callable::TCallable::Alias(target) => {
                            context.codebase.get_function_like(&canonical(*target)).is_some()
                        }
                        mago_codex::ttype::atomic::callable::TCallable::Signature(signature) => {
                            signature.source.is_some()
                        }
                    },
                )
            });
            let concrete_object = value.types.iter().any(|atomic| !atomic.get_all_object_names().is_empty())
                && parameter
                    .type_metadata
                    .as_ref()
                    .or(parameter.type_declaration_metadata.as_ref())
                    .is_none_or(|declared| declared.type_union != *value);
            (concrete_callable || concrete_object || ConditionValue::from_type(value).is_some())
                .then(|| (index, value.clone()))
        })
        .collect::<Vec<_>>();
    (trait_receiver || !arguments.is_empty()).then_some(ThrowsContext { function: canonical(function), arguments })
}

impl FunctionThrowsSummary {
    fn add(&mut self, exception: Word, condition: ThrowCondition) {
        let conditions = self.exceptions.entry(exception).or_default();
        if conditions.iter().any(|existing| existing.iter().all(|(key, value)| condition.get(key) == Some(value))) {
            return;
        }
        conditions.retain(|existing| !condition.iter().all(|(key, value)| existing.get(key) == Some(value)));
        if conditions.len() >= 64 {
            conditions.clear();
            conditions.push(ThrowCondition::new());
        } else {
            conditions.push(condition);
            conditions.sort();
        }
    }

    pub(crate) fn collect(block: &BlockContext<'_>, artifacts: &AnalysisArtifacts) -> Self {
        let mut summary = Self {
            unresolved_calls: block.unresolved_throw_calls.clone(),
            returned_generator: artifacts.returned_generator_throws.clone().map(Box::new),
            ..Self::default()
        };
        for (exception, spans) in &block.possibly_thrown_exceptions {
            for span in spans {
                let sites = summary.provenance.entry(*exception).or_default();
                if let Some(targets) = artifacts.throw_targets.get(&(*exception, *span)) {
                    sites.extend(targets.iter().map(|target| ThrowSite { span: *span, target: Some(*target) }));
                } else {
                    sites.push(ThrowSite { span: *span, target: None });
                }
                sites.sort();
                sites.dedup();
                if let Some(conditions) = artifacts.throw_conditions.get(&(*exception, *span)) {
                    for condition in conditions {
                        summary.add(*exception, condition.clone());
                    }
                } else {
                    summary.add(*exception, ThrowCondition::new());
                }
            }
        }
        summary
    }
}

fn current_condition(block: &BlockContext<'_>, artifacts: &AnalysisArtifacts) -> ThrowCondition {
    let mut condition = ThrowCondition::new();
    if let Some(metadata) = block.scope.get_function_like() {
        for (index, parameter) in metadata.parameters.iter().enumerate() {
            let variable = parameter.name.0;
            if block.assigned_variable_ids.get(&variable) != artifacts.throw_parameter_versions.get(&variable)
                || block.references_to_external_scope.contains(&variable)
                || parameter.flags.is_by_reference()
            {
                continue;
            }
            if let Some(value) = block.locals.get(&variable).and_then(|value| ConditionValue::from_type(value)) {
                condition.insert(index, value);
            }
        }
    }
    condition
}

pub(crate) fn record_throw(artifacts: &mut AnalysisArtifacts, block: &BlockContext<'_>, exception: Word, span: Span) {
    let condition = current_condition(block, artifacts);
    artifacts.throw_conditions.entry((exception, span)).or_default().push(condition);
}

pub(crate) fn report_global<A>(context: &mut Context<'_, '_, A>, block: &BlockContext<'_>)
where
    A: Arena,
{
    if !context.settings.check_throws_in_global_scope || context.throws_inference {
        return;
    }
    for (exception, spans) in &block.possibly_thrown_exceptions {
        let ignored = context
            .settings
            .unchecked_exception_classes_in_global_scope
            .iter()
            .any(|name| exception.as_bytes().eq_ignore_ascii_case(name.as_bytes()))
            || context.settings.unchecked_exceptions_in_global_scope.iter().any(|name| {
                exception.as_bytes().eq_ignore_ascii_case(name.as_bytes())
                    || context.codebase.is_instance_of(exception.as_bytes(), name.as_bytes())
            });
        if ignored {
            continue;
        }
        for span in spans {
            let issue = mago_reporting::Issue::error(format!("Uncaught exception `{exception}` in top-level code."))
                .with_annotation(mago_reporting::Annotation::primary(*span).with_message("Exception may escape here"));
            context.collector.report_with_code(crate::code::IssueCode::UncaughtThrowInGlobalScope, issue);
        }
    }
}

pub(crate) fn propagate<A>(
    context: &Context<'_, '_, A>,
    block: &mut BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'_, '_, '_>,
    summary: &FunctionThrowsSummary,
    parameters: &WordMap<TUnion>,
    target: FunctionLikeIdentifier,
) where
    A: Arena,
{
    let metadata = invocation.target.get_function_like_metadata().or_else(|| {
        invocation.target.get_function_like_identifier().and_then(|id| context.codebase.get_function_like(id))
    });
    for (exception, alternatives) in &summary.exceptions {
        for alternative in alternatives {
            let mut condition = current_condition(block, artifacts);
            let mut reachable = true;
            for (index, required) in alternative {
                let Some(parameter) = metadata.and_then(|metadata| metadata.parameters.get(*index)) else {
                    continue;
                };
                if let Some(actual) = parameters.get(&parameter.name.0)
                    && !can_expression_types_be_identical(context.codebase, actual, &required.union(), false, false)
                {
                    reachable = false;
                    break;
                }
                let argument =
                    invocation.arguments_source.iter_arguments().enumerate().find_map(|(position, argument)| {
                        if argument.is_unpacked() {
                            return None;
                        }
                        let matches = argument.get_parameter_name().map_or(position == *index, |name| {
                            parameter.name.0.as_bytes().strip_prefix(b"$") == Some(name)
                        });
                        matches.then(|| argument.value()).flatten()
                    });
                if let Some(Expression::Variable(Variable::Direct(variable))) = argument
                    && let Some(caller) = block.scope.get_function_like()
                    && let Some(caller_index) =
                        caller.parameters.iter().position(|parameter| parameter.name.0 == word(variable.name))
                    && block.assigned_variable_ids.get(&word(variable.name))
                        == artifacts.throw_parameter_versions.get(&word(variable.name))
                {
                    if let Some(existing) = condition.get(&caller_index)
                        && existing != required
                    {
                        reachable = false;
                        break;
                    }
                    condition.insert(caller_index, required.clone());
                }
            }
            if reachable {
                artifacts.throw_targets.entry((*exception, invocation.span)).or_default().insert(target);
                block.possibly_thrown_exceptions.entry(*exception).or_default().insert(invocation.span);
                artifacts.throw_conditions.entry((*exception, invocation.span)).or_default().push(condition);
            }
        }
    }
}

use crate::Analyzer;
use crate::analysis_result::AnalysisResult;
use crate::error::AnalysisError;
use crate::plugin::PluginRegistry;
use crate::settings::Settings;

/// Exceptions inferred from source bodies, independently of their PHPDoc contracts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ThrowsSummaries {
    pub yii2: yii2::Facts,
    pub functions: HashMap<FunctionLikeIdentifier, FunctionThrowsSummary>,
    pub source_files: HashSet<FileId>,
    pub contexts: HashMap<ThrowsContext, FunctionThrowsSummary>,
    pub dependencies: HashMap<FileId, HashSet<FileId>>,
}

impl ThrowsSummaries {
    fn reset_effects(&mut self, codebase: &CodebaseMetadata, files: &HashSet<FileId>) {
        for (id, summary) in &mut self.functions {
            if codebase.get_function_like(id).is_some_and(|metadata| files.contains(&metadata.span.file_id)) {
                *summary = FunctionThrowsSummary::default();
            }
        }
        for (key, summary) in &mut self.contexts {
            if function_metadata(codebase, &key.function).is_some_and(|metadata| files.contains(&metadata.span.file_id))
            {
                *summary = FunctionThrowsSummary::default();
            }
        }
    }

    /// Stabilizes body summaries before reporting callers. Only source files supplied
    /// by the host are analyzed. Other files continue to supply external contracts.
    /// Infer exception effects from project source bodies.
    ///
    /// # Errors
    /// Returns an analysis error if a provider or native analysis pass fails.
    pub fn infer(
        files: &[&File],
        codebase: &CodebaseMetadata,
        registry: &PluginRegistry,
        settings: &Settings,
        parser_settings: ParserSettings,
    ) -> Result<Self, AnalysisError> {
        Self::infer_incremental(files, codebase, registry, settings, parser_settings, Self::default(), None)
    }

    /// Reuse summaries after invalidating changed files and their callers.
    ///
    /// # Errors
    /// Returns an analysis error if a provider or native analysis pass fails.
    pub fn infer_incremental(
        files: &[&File],
        codebase: &CodebaseMetadata,
        registry: &PluginRegistry,
        settings: &Settings,
        parser_settings: ParserSettings,
        mut summaries: Self,
        affected: Option<&HashSet<FileId>>,
    ) -> Result<Self, AnalysisError> {
        let source_files = files.iter().map(|file| file.id).collect::<HashSet<_>>();
        let affected = affected.cloned().unwrap_or_else(|| source_files.clone());
        summaries.source_files = source_files;
        if registry.yii2_throws && !affected.is_empty() {
            summaries.yii2 = yii2::Facts::collect(files, codebase, parser_settings);
        }
        summaries.functions.retain(|id, _| codebase.get_function_like(id).is_some());
        summaries.contexts.retain(|key, _| function_metadata(codebase, &key.function).is_some());
        // Removed throws must not survive by circulating through cached recursive summaries.
        summaries.reset_effects(codebase, &affected);
        let mut inference_settings = settings.clone();
        inference_settings.diff = false;
        let parse_arena = LocalArena::new();
        let mut parsed_files = HashMap::default();
        let mut files_by_id = HashMap::default();
        for file in files {
            files_by_id.entry(file.id).or_insert(*file);
        }
        let trace_enabled = tracing::enabled!(tracing::Level::TRACE);
        let mut round = 0;
        let mut work = affected;
        let mut previous_functions: HashMap<FunctionLikeIdentifier, Option<FunctionThrowsSummary>> = HashMap::default();
        let mut previous_contexts: HashMap<ThrowsContext, Option<FunctionThrowsSummary>> = HashMap::default();
        loop {
            if work.is_empty() {
                return Ok(summaries);
            }
            round += 1;
            let round_started = std::time::Instant::now();
            tracing::trace!(
                round,
                files = work.len(),
                contexts = summaries.contexts.len(),
                "Starting throws inference round"
            );
            let mut next = summaries.clone();
            next.functions.retain(|id, _| {
                !codebase.get_function_like(id).is_some_and(|metadata| work.contains(&metadata.span.file_id))
            });
            next.contexts.retain(|key, _| {
                !function_metadata(codebase, &key.function)
                    .is_some_and(|metadata| work.contains(&metadata.span.file_id))
            });
            let mut requested_contexts = HashSet::default();
            let round_files = files.iter().copied().filter(|file| work.contains(&file.id)).collect::<Vec<_>>();
            // Parsed trees contain immutable slices, so workers can share them while
            // retaining the local parse arena for the entire inference operation.
            for file in &round_files {
                parsed_files.entry(file.id).or_insert_with(|| {
                    let program = parse_file_with_settings(&parse_arena, file, parser_settings);
                    (program, NameResolver::new(&parse_arena).resolve(program))
                });
            }
            let results = round_files
                .par_iter()
                .map_init(LocalArena::new, |arena, file| {
                    let started = trace_enabled.then(std::time::Instant::now);
                    let (program, names) = &parsed_files[&file.id];
                    let analyzer = Analyzer::new(arena, file, names, codebase, registry, inference_settings.clone())
                        .with_throws_summaries(&summaries)
                        .with_throws_inference();
                    let mut result = AnalysisResult::new(SymbolReferences::new());
                    let artifacts = analyzer.analyze_with_artifacts(program, &mut result);
                    arena.reset();
                    let artifacts = artifacts?;
                    let mut dependencies = artifacts.throws_dependencies;
                    collect_reference_files(&result.symbol_references, codebase, &mut dependencies);
                    if let Some(started) = started {
                        tracing::trace!(file = %mago_bytes::BytesDisplay(&file.name), elapsed = ?started.elapsed(), "Inferred general throws summaries");
                    }
                    Ok::<_, AnalysisError>((file.id, dependencies, artifacts.inferred_throws, artifacts.throws_context_requests))
                })
                .collect::<Vec<_>>();
            // Indexed collection preserves file/error order independently of workers.
            for result in results {
                let (file, dependencies, functions, contexts) = result?;
                next.dependencies.insert(file, dependencies);
                next.functions.extend(functions);
                requested_contexts.extend(contexts);
            }
            requested_contexts.extend(summaries.contexts.keys().cloned());
            let mut pending = requested_contexts.into_iter().collect::<std::collections::BTreeSet<_>>();
            let mut context_counts = HashMap::<FunctionLikeIdentifier, usize>::default();
            for key in next.contexts.keys() {
                *context_counts.entry(key.function).or_default() += 1;
            }
            let mut inferred_contexts = 0;
            let mut ready = HashMap::default();
            let workers = rayon::current_num_threads();
            let batch_size = if workers == 1 { 1 } else { workers.saturating_mul(16) };
            while let Some(specialization) = pending.pop_first() {
                if next.contexts.contains_key(&specialization) {
                    ready.remove(&specialization);
                    continue;
                }
                if context_counts.get(&specialization.function).copied().unwrap_or_default() >= 64 {
                    ready.remove(&specialization);
                    continue;
                }
                let Some(metadata) = function_metadata(codebase, &specialization.function) else {
                    continue;
                };
                let Some(file) = files_by_id.get(&metadata.span.file_id) else {
                    continue;
                };
                if !ready.contains_key(&specialization) {
                    // Calculate ahead, but commit only in the original priority-queue
                    // order. Newly discovered earlier keys must still get their turn
                    // before later keys, especially at the per-function context limit.
                    // Errors from contexts skipped by that limit are discarded too.
                    let batch = std::iter::once(specialization.clone())
                        .chain(
                            pending
                                .iter()
                                .filter(|key| {
                                    !ready.contains_key(*key)
                                        && !next.contexts.contains_key(*key)
                                        && context_counts.get(&key.function).copied().unwrap_or_default() < 64
                                        && function_metadata(codebase, &key.function)
                                            .is_some_and(|metadata| files_by_id.contains_key(&metadata.span.file_id))
                                })
                                .cloned(),
                        )
                        .take(batch_size)
                        .collect::<Vec<_>>();
                    for key in &batch {
                        if let Some(file) = function_metadata(codebase, &key.function)
                            .and_then(|metadata| files_by_id.get(&metadata.span.file_id))
                        {
                            parsed_files.entry(file.id).or_insert_with(|| {
                                let program = parse_file_with_settings(&parse_arena, file, parser_settings);
                                (program, NameResolver::new(&parse_arena).resolve(program))
                            });
                        }
                    }
                    let results = batch
                        .par_iter()
                        .map_init(LocalArena::new, |arena, key| {
                            let started = trace_enabled.then(std::time::Instant::now);
                            let metadata = function_metadata(codebase, &key.function)?;
                            let file = files_by_id.get(&metadata.span.file_id)?;
                            let (program, names) = &parsed_files[&file.id];
                            let mut analyzer =
                                Analyzer::new(arena, file, names, codebase, registry, inference_settings.clone())
                                    .with_throws_summaries(&summaries)
                                    .with_throws_inference();
                            analyzer.throws_specialization = Some(key);
                            let artifacts = analyzer
                                .analyze_with_artifacts(program, &mut AnalysisResult::new(SymbolReferences::new()));
                            arena.reset();
                            Some(artifacts.map(|artifacts| {
                                let summary = artifacts.inferred_throws.get(&key.function).cloned().unwrap_or_default();
                                (
                                    summary,
                                    artifacts.throws_dependencies,
                                    artifacts.throws_context_requests,
                                    started.map(|start| start.elapsed()),
                                )
                            }))
                        })
                        .collect::<Vec<_>>();
                    ready.extend(
                        batch.into_iter().zip(results).filter_map(|(key, result)| result.map(|result| (key, result))),
                    );
                }
                let Some(result) = ready.remove(&specialization) else {
                    continue;
                };
                let (summary, dependencies, requests, elapsed) = result?;
                next.dependencies.entry(file.id).or_default().extend(dependencies);
                pending.extend(requests.into_iter().filter(|key| !next.contexts.contains_key(key)));
                *context_counts.entry(specialization.function).or_default() += 1;
                inferred_contexts += 1;
                if let Some(elapsed) = elapsed {
                    tracing::trace!(function = ?specialization.function, elapsed = ?elapsed, "Inferred throws context");
                }
                next.contexts.insert(specialization, summary);
            }
            tracing::trace!(round, inferred_contexts, contexts = next.contexts.len(), elapsed = ?round_started.elapsed(), "Completed throws inference round");
            if next == summaries {
                return Ok(next);
            }
            let mut changed = HashSet::default();
            let mut changed_functions = HashMap::default();
            let mut changed_contexts = HashMap::default();
            for (id, summary) in &next.functions {
                if summaries.functions.get(id) != Some(summary)
                    && let Some(metadata) = codebase.get_function_like(id)
                {
                    changed_functions.insert(*id, summaries.functions.get(id).cloned());
                    if trace_enabled && (32..36).contains(&round) && work.len() <= 16 {
                        tracing::trace!(function = ?id, previous = ?summaries.functions.get(id), current = ?summary, "Changed throws function summary");
                    }
                    changed.insert(metadata.span.file_id);
                }
            }
            for (key, summary) in &next.contexts {
                if summaries.contexts.get(key) != Some(summary)
                    && let Some(metadata) = function_metadata(codebase, &key.function)
                {
                    changed_contexts.insert(key.clone(), summaries.contexts.get(key).cloned());
                    if trace_enabled && (32..36).contains(&round) && work.len() <= 16 {
                        tracing::trace!(function = ?key.function, arguments = ?key.arguments, previous = ?summaries.contexts.get(key), current = ?summary, "Changed throws context summary");
                    }
                    changed.insert(metadata.span.file_id);
                }
            }
            work = next
                .dependencies
                .iter()
                .filter(|(_, callees)| !callees.is_disjoint(&changed))
                .map(|(file, _)| *file)
                .collect();
            work.extend(changed);
            let reverses_previous_round = (!changed_functions.is_empty() || !changed_contexts.is_empty())
                && changed_functions.len() == previous_functions.len()
                && changed_contexts.len() == previous_contexts.len()
                && changed_functions.keys().all(|id| {
                    previous_functions.get(id).is_some_and(|previous| previous.as_ref() == next.functions.get(id))
                })
                && changed_contexts.keys().all(|key| {
                    previous_contexts.get(key).is_some_and(|previous| previous.as_ref() == next.contexts.get(key))
                });
            if reverses_previous_round {
                // New specializations can inherit general effects before their callees are registered.
                // Recompute oscillating summaries from empty effects with their keys retained.
                // Known bodies, external contracts and unresolved calls are analyzed again normally.
                for id in changed_functions.keys() {
                    if let Some(summary) = next.functions.get_mut(id) {
                        *summary = FunctionThrowsSummary::default();
                    }
                }
                for key in changed_contexts.keys() {
                    if let Some(summary) = next.contexts.get_mut(key) {
                        *summary = FunctionThrowsSummary::default();
                    }
                }
                tracing::debug!(
                    round,
                    files = work.len(),
                    functions = changed_functions.len(),
                    contexts = changed_contexts.len(),
                    "Recomputing cyclic throws effects from registered contexts"
                );
                previous_functions.clear();
                previous_contexts.clear();
            } else {
                previous_functions = changed_functions;
                previous_contexts = changed_contexts;
            }
            summaries = next;
        }
    }
}

fn collect_reference_files(references: &SymbolReferences, codebase: &CodebaseMetadata, files: &mut HashSet<FileId>) {
    references.for_each_reference(|_, (symbol, member), _| {
        if let Some(class) = codebase.get_class_like(symbol.as_bytes()) {
            files.insert(class.span.file_id);
        }
        if let Some(function) = codebase.function_likes.get(&(symbol, member)) {
            files.insert(function.span.file_id);
        }
        if member.is_empty()
            && let Some(constant) = codebase.get_constant(symbol.as_bytes())
        {
            files.insert(constant.span.file_id);
        }
    });
}
