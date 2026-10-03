use mago_allocator::Arena;
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::ttype::comparator::union_comparator::can_expression_types_be_identical;
use mago_codex::ttype::{get_literal_int, union::TUnion};
use mago_word::{Word, WordMap};

use crate::{context::Context, invocation::Invocation};

pub(crate) fn possible<A: Arena>(
    context: &Context<'_, '_, A>,
    invocation: &Invocation<'_, '_, '_>,
    parameters: &WordMap<TUnion>,
    exception: Word,
) -> bool {
    if !matches!(invocation.target.get_function_like_identifier(), Some(FunctionLikeIdentifier::Function(name)) if name.as_bytes() == b"intdiv")
    {
        return true;
    }
    let can_be = |index: usize, value: i64| {
        invocation
            .target
            .get_function_like_metadata()
            .and_then(|metadata| metadata.parameters.get(index))
            .and_then(|parameter| parameters.get(&parameter.name.0))
            .is_none_or(|actual| {
                can_expression_types_be_identical(context.codebase, actual, &get_literal_int(value), false, false)
            })
    };
    match exception.as_bytes() {
        b"DivisionByZeroError" => can_be(1, 0),
        b"ArithmeticError" => can_be(1, -1) && (can_be(0, i64::MIN) || can_be(0, i64::from(i32::MIN))),
        _ => true,
    }
}
