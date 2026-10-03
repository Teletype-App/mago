use mago_allocator::Arena;
use std::rc::Rc;

use mago_codex::ttype::TType;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::combine_union_types;
use mago_codex::ttype::get_never;
use mago_reporting::Annotation;
use mago_reporting::Issue;
use mago_span::HasSpan;
use mago_syntax::cst::Throw;

use crate::analyzable::Analyzable;
use crate::artifacts::AnalysisArtifacts;
use crate::code::IssueCode;
use crate::context::Context;
use crate::context::block::BlockContext;
use crate::error::AnalysisError;

impl<'ast, 'arena> Analyzable<'ast, 'arena> for Throw<'arena> {
    fn analyze<'ctx, A>(
        &'ast self,
        context: &mut Context<'ctx, 'arena, A>,
        block_context: &mut BlockContext<'ctx>,
        artifacts: &mut AnalysisArtifacts,
    ) -> Result<(), AnalysisError>
    where
        A: Arena,
    {
        let was_inside_throw = block_context.flags.inside_throw();
        block_context.flags.set_inside_throw(true);
        self.exception.analyze(context, block_context, artifacts)?;
        block_context.flags.set_inside_throw(was_inside_throw);
        block_context.flags.set_has_returned(true);
        if let Some(scope) = block_context.finally_scope.as_ref() {
            let mut finally_scope = scope.borrow_mut();

            for (variable, previous_type) in &block_context.locals {
                if let Some(finally_type) = finally_scope.locals.get_mut(variable) {
                    let resulting_type = combine_union_types(
                        previous_type.as_ref(),
                        finally_type.as_ref(),
                        context.codebase,
                        context.settings.combiner_options(),
                    );

                    finally_scope.locals.insert(*variable, Rc::new(resulting_type));
                } else {
                    let mut resulting_type = (**previous_type).clone();
                    resulting_type.set_possibly_undefined_from_try(true);

                    finally_scope.locals.insert(*variable, Rc::new(resulting_type));
                }
            }
        }

        let mut thrown_names = Vec::new();
        if let Some(exception_type) = artifacts.get_expression_type(self.exception) {
            for exception_atomic in exception_type.types.as_ref() {
                if exception_atomic.extends_or_implements(context.codebase, b"Throwable") {
                    let thrown_types = match exception_atomic {
                        TAtomic::GenericParameter(parameter) => parameter.constraint.types.as_ref(),
                        _ => std::slice::from_ref(exception_atomic),
                    };
                    for object_name in thrown_types.iter().flat_map(TAtomic::get_all_object_names) {
                        if !context.codebase.is_instance_of(object_name.as_bytes(), b"Throwable")
                            && !object_name.as_bytes().eq_ignore_ascii_case(b"Throwable")
                        {
                            continue;
                        }
                        block_context.possibly_thrown_exceptions.entry(object_name).or_default().insert(self.span());
                        thrown_names.push(object_name);
                    }
                } else {
                    let exception_atomic_str = exception_atomic.get_id();

                    context.collector.report_with_code(
                        IssueCode::InvalidThrow,
                        Issue::error(format!(
                            "Cannot throw type `{exception_atomic_str}` because it is not an instance of Throwable."
                        ))
                        .with_annotation(
                            Annotation::primary(self.span())
                                .with_message(format!("This has type `{exception_atomic_str}`, not `Throwable`"))
                        )
                        .with_note(
                            "Only objects that implement the `Throwable` interface (like `Exception` or `Error`) can be thrown."
                        )
                        .with_help(
                            "Ensure the value being thrown is an instance of `Exception`, `Error`, or a subclass thereof."
                        ),
                    );
                }
            }
        }

        if context.settings.throws_enabled() {
            for exception in thrown_names {
                crate::throws::record_throw(artifacts, block_context, exception, self.span());
            }
        }
        artifacts.set_expression_type(self, get_never());

        Ok(())
    }
}
