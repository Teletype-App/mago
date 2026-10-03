use std::collections::BTreeMap;

use mago_allocator::{Arena, LocalArena};
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::identifier::method::MethodIdentifier;
use mago_codex::metadata::CodebaseMetadata;
use mago_database::file::File;
use mago_names::{ResolvedNames, resolver::NameResolver};
use mago_syntax::cst::{
    Array, ArrayElement, Class, ClassLikeConstantSelector, ClassLikeMember, ClassLikeMemberSelector, Expression,
    Literal, MethodCall, Variable,
};
use mago_syntax::parser::parse_file_with_settings;
use mago_syntax::settings::ParserSettings;
use mago_syntax::walker::MutWalker;
use mago_word::{Word, WordMap, ascii_lowercase_word, word};

use super::ThrowsSummaries;
use crate::context::Context;
use crate::invocation::Invocation;
use crate::plugin::context::InvocationInfo;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Facts {
    #[cfg_attr(feature = "serde", serde(with = "super::word_map_serde"))]
    validators: WordMap<Vec<FunctionLikeIdentifier>>,
    #[cfg_attr(feature = "serde", serde(with = "super::word_map_serde"))]
    handlers: WordMap<BTreeMap<String, Vec<FunctionLikeIdentifier>>>,
}

impl Facts {
    pub(super) fn collect(files: &[&File], codebase: &CodebaseMetadata, settings: ParserSettings) -> Self {
        let mut facts = Self::default();
        for file in files {
            let arena = LocalArena::new();
            let program = parse_file_with_settings(&arena, file, settings);
            let names = NameResolver::new(&arena).resolve(program);
            ClassCollector { facts: &mut facts, codebase, names: &names }.walk_program(program, &mut ());
        }
        facts
    }
}

struct ClassCollector<'facts, 'arena> {
    facts: &'facts mut Facts,
    codebase: &'facts CodebaseMetadata,
    names: &'facts ResolvedNames<'arena>,
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for ClassCollector<'_, 'arena> {
    fn walk_in_class(&mut self, class: &'ast Class<'arena>, _: &mut ()) {
        let name = ascii_lowercase_word(self.names.get(&class.name));
        for member in class.members.iter() {
            let ClassLikeMember::Method(method) = member else {
                continue;
            };
            let rules = method.name.value.eq_ignore_ascii_case(b"rules")
                || method.name.value.eq_ignore_ascii_case(b"rulesList");
            MemberCollector { facts: self.facts, codebase: self.codebase, names: self.names, class: name, rules }
                .walk_method(method, &mut ());
        }
    }
}

struct MemberCollector<'facts, 'arena> {
    facts: &'facts mut Facts,
    codebase: &'facts CodebaseMetadata,
    names: &'facts ResolvedNames<'arena>,
    class: Word,
    rules: bool,
}

fn string<'arena>(expression: &Expression<'arena>) -> Option<&'arena [u8]> {
    if let Expression::Literal(Literal::String(value)) = expression { value.value } else { None }
}

fn class_name(
    expression: &Expression<'_>,
    class: Word,
    codebase: &CodebaseMetadata,
    names: &ResolvedNames<'_>,
) -> Option<Word> {
    match expression {
        Expression::Variable(Variable::Direct(variable)) if variable.name == b"$this" => Some(class),
        Expression::Literal(Literal::String(value)) => value.value.map(ascii_lowercase_word),
        Expression::Access(mago_syntax::cst::Access::ClassConstant(access)) => {
            class_name(access.class, class, codebase, names)
        }
        Expression::Identifier(identifier) => {
            Some(ascii_lowercase_word(names.resolve(identifier).unwrap_or(identifier.value())))
        }
        Expression::Self_(_) | Expression::Static(_) => Some(class),
        Expression::Parent(_) => codebase.get_class_like(class.as_bytes())?.direct_parent_class,
        _ => None,
    }
}

fn callback(
    expression: &Expression<'_>,
    class: Word,
    codebase: &CodebaseMetadata,
    names: &ResolvedNames<'_>,
) -> Option<FunctionLikeIdentifier> {
    let elements = match expression {
        Expression::Array(array) => &array.elements,
        Expression::LegacyArray(array) => &array.elements,
        _ => return None,
    };
    if elements.len() != 2 {
        return None;
    }
    let ArrayElement::Value(receiver) = elements.get(0)? else {
        return None;
    };
    let ArrayElement::Value(method) = elements.get(1)? else {
        return None;
    };
    Some(FunctionLikeIdentifier::Method(
        class_name(receiver.value, class, codebase, names)?,
        ascii_lowercase_word(string(method.value)?),
    ))
}

fn event_key(
    expression: &Expression<'_>,
    class: Word,
    codebase: &CodebaseMetadata,
    names: &ResolvedNames<'_>,
) -> Option<String> {
    if let Some(value) = string(expression) {
        return Some(format!("string:{}", String::from_utf8_lossy(value)));
    }
    let Expression::Access(mago_syntax::cst::Access::ClassConstant(access)) = expression else {
        return None;
    };
    let ClassLikeConstantSelector::Identifier(constant) = &access.constant else {
        return None;
    };
    Some(format!(
        "const:{}::{}",
        class_name(access.class, class, codebase, names)?,
        ascii_lowercase_word(constant.value)
    ))
}

impl MemberCollector<'_, '_> {
    fn collect_rules(&mut self, expression: &Expression<'_>) {
        if !self.rules {
            return;
        }
        if let Some(target @ FunctionLikeIdentifier::Method(class, _)) =
            callback(expression, self.class, self.codebase, self.names)
            && self.codebase.get_class_like(class.as_bytes()).is_some()
        {
            let targets = self.facts.validators.entry(self.class).or_default();
            if !targets.contains(&target) {
                targets.push(target);
                targets.sort();
            }
            return;
        }
        let elements = match expression {
            Expression::Array(array) => &array.elements,
            Expression::LegacyArray(array) => &array.elements,
            _ => return,
        };
        for element in elements
            .iter()
            .filter_map(|element| if let ArrayElement::Value(value) = element { Some(value.value) } else { None })
            .take(2)
        {
            let Some(name) = string(element) else {
                continue;
            };
            if self.codebase.get_method(self.class.as_bytes(), name).is_some() {
                let target = FunctionLikeIdentifier::Method(self.class, ascii_lowercase_word(name));
                let targets = self.facts.validators.entry(self.class).or_default();
                if !targets.contains(&target) {
                    targets.push(target);
                    targets.sort();
                }
            }
        }
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for MemberCollector<'_, 'arena> {
    fn walk_in_array(&mut self, array: &'ast Array<'arena>, _: &mut ()) {
        self.collect_rules(&Expression::Array(array.clone()));
    }
    fn walk_in_legacy_array(&mut self, array: &'ast mago_syntax::cst::LegacyArray<'arena>, _: &mut ()) {
        self.collect_rules(&Expression::LegacyArray(array.clone()));
    }

    fn walk_in_method_call(&mut self, call: &'ast MethodCall<'arena>, _: &mut ()) {
        let Expression::Variable(Variable::Direct(receiver)) = call.object else {
            return;
        };
        let ClassLikeMemberSelector::Identifier(method) = &call.method else {
            return;
        };
        if receiver.name != b"$this" || !method.value.eq_ignore_ascii_case(b"on") {
            return;
        }
        let args = call.argument_list.arguments.iter().map(|argument| argument.value()).collect::<Vec<_>>();
        let Some(key) =
            args.first().and_then(|expression| event_key(expression, self.class, self.codebase, self.names))
        else {
            return;
        };
        let Some(target) =
            args.get(1).and_then(|expression| callback(expression, self.class, self.codebase, self.names))
        else {
            return;
        };
        let targets = self.facts.handlers.entry(self.class).or_default().entry(key).or_default();
        if !targets.contains(&target) {
            targets.push(target);
            targets.sort();
        }
    }
}

pub(crate) fn targets<A>(
    context: &Context<'_, '_, A>,
    artifacts: &crate::artifacts::AnalysisArtifacts,
    invocation: &Invocation<'_, '_, '_>,
) -> Vec<FunctionLikeIdentifier>
where
    A: Arena,
{
    let Some(method) = invocation.target.get_method_context() else {
        return Vec::new();
    };
    let Some(FunctionLikeIdentifier::Method(_, name)) = invocation.target.get_function_like_identifier() else {
        return Vec::new();
    };
    let Some(summaries) = context.throws_summaries else {
        return Vec::new();
    };
    let class = method.class_like_metadata.name;
    if !is_framework_class(context.codebase, class) {
        return Vec::new();
    }
    let info = InvocationInfo::new(invocation);
    let argument_type = |index, names: &[&[u8]]| {
        info.get_argument(index, names).and_then(|expression| artifacts.get_expression_type(expression))
    };
    let mut hooks: Vec<&[u8]> = Vec::new();
    let mut result = Vec::new();
    let name = ascii_lowercase_word(name.as_bytes());
    let validates = !argument_type(0, &[b"runValidation"]).is_some_and(|value| value.is_false());
    match name.as_bytes() {
        b"__construct" => hooks.push(b"init"),
        b"validate" => hooks.extend([b"beforeValidate".as_slice(), b"afterValidate".as_slice()]),
        b"save" | b"insert" | b"update" => {
            if validates {
                hooks.extend([b"beforeValidate".as_slice(), b"afterValidate".as_slice()]);
            }
            hooks.extend([b"beforeSave".as_slice(), b"afterSave".as_slice()]);
        }
        b"delete" => hooks.extend([b"beforeDelete".as_slice(), b"afterDelete".as_slice()]),
        b"findone" | b"findall" | b"populaterecord" => hooks.push(b"afterFind"),
        b"refresh" => hooks.push(b"afterRefresh"),
        b"createobject" => {
            if let Some(expression) = info.get_argument(0, &[b"type"]) {
                let created = argument_type(0, &[b"type"])
                    .and_then(|ty| {
                        ty.get_single_class_string_value().or_else(|| ty.get_single_literal_string_value().map(word))
                    })
                    .or_else(|| config_class(expression, class, context));
                if let Some(created) = created {
                    resolve(context.codebase, summaries, created, &[b"__construct", b"init"], true, &mut result);
                }
            }
        }
        b"setattributes" if argument_type(1, &[b"safeOnly"]).is_some_and(|value| value.is_false()) => {
            if let Some(Expression::Array(array)) = info.get_argument(0, &[b"values"]) {
                for element in array.elements.iter() {
                    let ArrayElement::KeyValue(pair) = element else {
                        continue;
                    };
                    let Some(property) = string(pair.key) else {
                        continue;
                    };
                    if context
                        .codebase
                        .get_declaring_property(class.as_bytes(), mago_word::concat_word!(b"$", property).as_bytes())
                        .is_some()
                    {
                        continue;
                    }
                    let mut setter = b"set".to_vec();
                    setter.extend(property);
                    resolve(context.codebase, summaries, class, &[&setter], false, &mut result);
                }
            }
        }
        _ => {}
    }
    resolve(context.codebase, summaries, class, &hooks, true, &mut result);
    let classes = std::iter::once(class).chain(method.class_like_metadata.all_parent_classes.iter().copied());
    for inherited in classes {
        let inherited = ascii_lowercase_word(inherited.as_bytes());
        if (name.as_bytes() == b"validate" || (matches!(name.as_bytes(), b"save" | b"insert" | b"update") && validates))
            && let Some(validators) = summaries.yii2.validators.get(&inherited)
        {
            for target in validators {
                if let FunctionLikeIdentifier::Method(receiver, validator) = target {
                    resolve(
                        context.codebase,
                        summaries,
                        if *receiver == inherited { class } else { *receiver },
                        &[validator.as_bytes()],
                        true,
                        &mut result,
                    );
                }
            }
        }
        if name.as_bytes() == b"trigger"
            && let Some(expression) = info.get_argument(0, &[b"name"])
        {
            let key = event_key(expression, class, context.codebase, context.resolved_names);
            if let Some(handlers) = key.as_ref().and_then(|key| summaries.yii2.handlers.get(&inherited)?.get(key)) {
                for target in handlers {
                    if let FunctionLikeIdentifier::Method(receiver, handler) = target {
                        resolve(
                            context.codebase,
                            summaries,
                            if *receiver == inherited { class } else { *receiver },
                            &[handler.as_bytes()],
                            false,
                            &mut result,
                        );
                    }
                }
            }
        }
    }
    let execution: Option<&[u8]> = match name.as_bytes() {
        b"all" | b"findall" => Some(b"queryAll"),
        b"one" | b"findone" | b"refresh" => Some(b"queryOne"),
        b"column" => Some(b"queryColumn"),
        b"scalar" | b"count" | b"sum" | b"average" | b"min" | b"max" | b"exists" => Some(b"queryScalar"),
        b"updateall" | b"updateallcounters" | b"deleteall" => Some(b"execute"),
        _ => None,
    };
    if let Some(execution) = execution {
        resolve(context.codebase, summaries, word(b"yii\\db\\Command"), &[execution], false, &mut result);
    }
    result.sort();
    result.dedup();
    result
}

fn is_framework_class(codebase: &CodebaseMetadata, class: Word) -> bool {
    [
        b"yii\\base\\BaseObject".as_slice(),
        b"yii\\base\\Component",
        b"yii\\base\\Model",
        b"yii\\db\\BaseActiveRecord",
        b"yii\\db\\QueryInterface",
        b"yii\\db\\Query",
        b"yii\\db\\ActiveQuery",
        b"yii\\BaseYii",
    ]
    .iter()
    .any(|base| class.as_bytes().eq_ignore_ascii_case(base) || codebase.is_instance_of(class.as_bytes(), base))
}

pub(crate) fn collect_property<A: Arena>(
    context: &Context<'_, '_, A>,
    block: &mut crate::context::block::BlockContext<'_>,
    artifacts: &mut crate::artifacts::AnalysisArtifacts,
    class: Word,
    property: Word,
    span: mago_span::Span,
    write: bool,
) {
    if !context.settings.throws_enabled()
        || !context.plugin_registry.yii2_throws
        || !is_framework_class(context.codebase, class)
        || context.codebase.get_declaring_property(class.as_bytes(), property.as_bytes()).is_some()
    {
        return;
    }
    let Some(summaries) = context.throws_summaries else {
        return;
    };
    let mut method = if write { b"set".to_vec() } else { b"get".to_vec() };
    method.extend(property.as_bytes().strip_prefix(b"$").unwrap_or(property.as_bytes()));
    let mut targets = Vec::new();
    resolve(context.codebase, summaries, class, &[&method], false, &mut targets);
    for target in targets {
        let Some(metadata) = context.codebase.get_function_like(&target) else {
            continue;
        };
        artifacts.throws_dependencies.insert(metadata.span.file_id);
        let summary = summaries.functions.get(&target);
        if let Some(summary) = summary {
            for exception in summary.exceptions.keys() {
                block.possibly_thrown_exceptions.entry(*exception).or_default().insert(span);
                artifacts.throw_targets.entry((*exception, span)).or_default().insert(target);
                super::record_throw(artifacts, block, *exception, span);
            }
            if !summary.unresolved_calls.is_empty() {
                block.unresolved_throw_calls.insert(span);
            }
        } else if !summaries.source_files.contains(&metadata.span.file_id) {
            for exception in metadata
                .thrown_types
                .iter()
                .flat_map(|thrown| thrown.type_union.types.iter())
                .flat_map(mago_codex::ttype::atomic::TAtomic::get_all_object_names)
            {
                block.possibly_thrown_exceptions.entry(exception).or_default().insert(span);
                artifacts.throw_targets.entry((exception, span)).or_default().insert(target);
                super::record_throw(artifacts, block, exception, span);
            }
        }
    }
}

fn config_class<A>(expression: &Expression<'_>, class: Word, context: &Context<'_, '_, A>) -> Option<Word>
where
    A: Arena,
{
    let Expression::Array(array) = expression else {
        return class_name(expression, class, context.codebase, context.resolved_names);
    };
    array.elements.iter().find_map(|element| {
        let ArrayElement::KeyValue(pair) = element else {
            return None;
        };
        (string(pair.key) == Some(b"class"))
            .then(|| class_name(pair.value, class, context.codebase, context.resolved_names))
            .flatten()
    })
}

fn resolve(
    codebase: &CodebaseMetadata,
    summaries: &ThrowsSummaries,
    class: Word,
    methods: &[&[u8]],
    project_only: bool,
    result: &mut Vec<FunctionLikeIdentifier>,
) {
    for method in methods {
        if codebase.get_method(class.as_bytes(), method).is_none() {
            continue;
        }
        let identifier = super::canonical(
            codebase.get_declaring_method_identifier(&MethodIdentifier::new(class, word(method))).into(),
        );
        if !project_only
            || codebase
                .get_function_like(&identifier)
                .is_some_and(|metadata| summaries.source_files.contains(&metadata.span.file_id))
        {
            result.push(identifier);
        }
    }
}
