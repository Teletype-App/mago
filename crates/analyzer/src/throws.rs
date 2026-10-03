pub(crate) mod callbacks;
pub(crate) mod docblock;
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
    let function = invocation
        .target
        .get_method_context()
        .and_then(|method| method.declaring_method_id)
        .map(FunctionLikeIdentifier::from)
        .or_else(|| invocation.target.get_function_like_identifier().copied())?;
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
    (!arguments.is_empty()).then_some(ThrowsContext { function: canonical(function), arguments })
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
        let mut summary = Self { unresolved_calls: block.unresolved_throw_calls.clone(), ..Self::default() };
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
        summaries.contexts.retain(|key, _| codebase.get_function_like(&key.function).is_some());
        let mut inference_settings = settings.clone();
        inference_settings.diff = false;
        let mut work = affected;
        loop {
            if work.is_empty() {
                return Ok(summaries);
            }
            let mut next = summaries.clone();
            next.functions.retain(|id, _| {
                !codebase.get_function_like(id).is_some_and(|metadata| work.contains(&metadata.span.file_id))
            });
            next.contexts.retain(|key, _| {
                !codebase.get_function_like(&key.function).is_some_and(|metadata| work.contains(&metadata.span.file_id))
            });
            let mut requested_contexts = HashSet::default();
            for file in files.iter().filter(|file| work.contains(&file.id)) {
                let arena = LocalArena::new();
                let program = parse_file_with_settings(&arena, file, parser_settings);
                let names = NameResolver::new(&arena).resolve(program);
                let analyzer = Analyzer::new(&arena, file, &names, codebase, registry, inference_settings.clone())
                    .with_throws_summaries(&summaries)
                    .with_throws_inference();
                let mut result = AnalysisResult::new(SymbolReferences::new());
                let artifacts = analyzer.analyze_with_artifacts(program, &mut result)?;
                let mut dependencies = artifacts.throws_dependencies;
                collect_reference_files(&result.symbol_references, codebase, &mut dependencies);
                next.dependencies.insert(file.id, dependencies);
                next.functions.extend(artifacts.inferred_throws);
                requested_contexts.extend(artifacts.throws_context_requests);
            }
            requested_contexts.extend(summaries.contexts.keys().cloned());
            let mut pending = requested_contexts.into_iter().collect::<std::collections::BTreeSet<_>>();
            while let Some(specialization) = pending.pop_first() {
                if next.contexts.contains_key(&specialization) {
                    continue;
                }
                if next.contexts.keys().filter(|key| key.function == specialization.function).count() >= 64 {
                    continue;
                }
                let Some(metadata) = codebase.get_function_like(&specialization.function) else {
                    continue;
                };
                let Some(file) = files.iter().find(|file| file.id == metadata.span.file_id) else {
                    continue;
                };
                let arena = LocalArena::new();
                let program = parse_file_with_settings(&arena, file, parser_settings);
                let names = NameResolver::new(&arena).resolve(program);
                let mut analyzer = Analyzer::new(&arena, file, &names, codebase, registry, inference_settings.clone())
                    .with_throws_summaries(&summaries)
                    .with_throws_inference();
                analyzer.throws_specialization = Some(&specialization);
                let artifacts =
                    analyzer.analyze_with_artifacts(program, &mut AnalysisResult::new(SymbolReferences::new()))?;
                let summary = artifacts.inferred_throws.get(&specialization.function).cloned().unwrap_or_default();
                next.dependencies.entry(file.id).or_default().extend(artifacts.throws_dependencies);
                pending.extend(
                    artifacts.throws_context_requests.into_iter().filter(|key| !next.contexts.contains_key(key)),
                );
                next.contexts.insert(specialization, summary);
            }
            if next == summaries {
                return Ok(next);
            }
            let mut changed = HashSet::default();
            for (id, summary) in &next.functions {
                if summaries.functions.get(id) != Some(summary)
                    && let Some(metadata) = codebase.get_function_like(id)
                {
                    changed.insert(metadata.span.file_id);
                }
            }
            for (key, summary) in &next.contexts {
                if summaries.contexts.get(key) != Some(summary)
                    && let Some(metadata) = codebase.get_function_like(&key.function)
                {
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
