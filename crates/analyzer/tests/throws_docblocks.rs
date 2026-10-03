#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;
use std::sync::LazyLock;

use foldhash::HashSet;
use mago_allocator::LocalArena;
use mago_analyzer::Analyzer;
use mago_analyzer::analysis_result::AnalysisResult;
use mago_analyzer::plugin::PluginRegistry;
use mago_analyzer::settings::Settings;
use mago_codex::populator::populate_codebase;
use mago_codex::scanner::scan_program;
use mago_database::file::File;
use mago_names::resolver::NameResolver;
use mago_prelude::Prelude;
use mago_reporting::IssueCollection;
use mago_syntax::parser::parse_file;
use mago_text_edit::{ApplyResult, Safety, TextEditor};
use mago_word::WordSet;

static PRELUDE: LazyLock<Prelude> = LazyLock::new(Prelude::build);

fn analyze(source: &str) -> IssueCollection {
    let Prelude { mut metadata, mut symbol_references, .. } = PRELUDE.clone();
    let file = File::ephemeral(Cow::Borrowed(b"contract.php"), Cow::Owned(source.as_bytes().to_vec()));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    assert!(!program.has_errors(), "Invalid PHP: {:?}", program.errors);
    let names = NameResolver::new(&arena).resolve(program);
    let settings = Settings { check_throws: true, find_unused_parameters: false, ..Settings::default() };
    metadata.extend(scan_program(&arena, &file, program, &names, settings.version));
    populate_codebase(&mut metadata, &mut symbol_references, WordSet::default(), HashSet::default());
    let registry = PluginRegistry::with_library_providers();
    let mut result = AnalysisResult::new(symbol_references);
    Analyzer::new(&arena, &file, &names, &metadata, &registry, settings).analyze(program, &mut result).unwrap();
    result.issues.extend(metadata.take_issues(true));
    result.issues
}

fn fix(source: &str) -> String {
    let issues = analyze(source);
    let mut editor = TextEditor::with_safety(source.as_bytes(), Safety::PotentiallyUnsafe);
    for issue in issues {
        for edits in issue.edits.into_values() {
            assert_eq!(ApplyResult::Applied, editor.apply_batch(edits, None::<fn(&[u8]) -> bool>));
        }
    }
    String::from_utf8(editor.finish()).unwrap()
}

#[test]
fn addition_preserves_imports_description_and_other_tags() {
    let source = "<?php\nnamespace Contracts;\nuse DomainException as Fault;\n/** Does something.\n * @param bool $raise Keep this description.\n */\nfunction run(bool $raise): void { if ($raise) { throw new Fault(); } }\n";
    let fixed = fix(source);
    assert_eq!(fixed, source.replace(" */", " * @throws \\DomainException\n */"));
    assert!(analyze(&fixed).is_empty());
    assert_eq!(fix(&fixed), fixed);
}

#[test]
fn narrowing_preserves_exception_description() {
    let source = "<?php\n/** @throws Throwable When processing fails. */\nfunction run(): void { throw new DomainException(); }\n";
    assert_eq!(fix(source), source.replace("@throws Throwable", "@throws \\DomainException"));
}

#[test]
fn removal_preserves_descriptions_and_non_throws_tags() {
    let source = "<?php\n/** Documentation.\n * @throws RuntimeException Historical explanation.\n * @return void\n */\nfunction run(): void {}\n";
    assert_eq!(fix(source), source.replace("@throws RuntimeException ", ""));
    let without_description = source.replace("RuntimeException Historical explanation.", "RuntimeException");
    assert_eq!(fix(&without_description), without_description.replace(" * @throws RuntimeException\n", ""));
    let inherited = "<?php
interface Contract { /** @throws RuntimeException */ public function run(): void; }
final class Worker implements Contract { public function run(): void {} }
";
    assert!(analyze(inherited).is_empty());
    assert_eq!(fix(inherited), inherited);
}

#[test]
fn addition_to_inline_docblock_is_parseable_and_idempotent() {
    let source = "<?php\n/** @param bool $raise */\nfunction run(bool $raise): void { if ($raise) { throw new DomainException(); } }\n";
    let expected = "<?php\n/**\n * @param bool $raise\n * @throws \\DomainException\n */\nfunction run(bool $raise): void { if ($raise) { throw new DomainException(); } }\n";
    assert_eq!(fix(source), expected);
    assert!(analyze(expected).is_empty());
    assert_eq!(fix(expected), expected);
}

#[test]
fn unresolved_callback_preserves_contract_and_reports_incomplete_inference() {
    let source = "<?php\n/**\n * @param callable(): void $callback\n * @throws RuntimeException\n */\nfunction run(callable $callback): void { $callback(); }\n";
    let issues = analyze(source);
    assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("throws-inference-incomplete")));
    assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some("unused-throws-type")));
    assert_eq!(fix(source), source);
}

#[test]
fn crlf_and_method_indentation_are_preserved() {
    let source =
        "<?php\r\nfinal class Worker {\r\n    public function run(): void { throw new DomainException(); }\r\n}\r\n";
    let expected = source.replace(
        "    public function",
        "    /**\r\n     * @throws \\DomainException\r\n     */\r\n    public function",
    );
    assert_eq!(fix(source), expected);
    assert!(analyze(&expected).is_empty());
}

fn missing_for(source: &str, function: &str) -> Vec<String> {
    analyze(source)
        .into_iter()
        .filter(|issue| {
            issue.code.as_deref() == Some("unhandled-thrown-type") && issue.message.contains(&format!("`{function}`"))
        })
        .map(|issue| issue.message)
        .collect()
}

#[test]
fn impossible_branches_do_not_contribute_exception_effects() {
    let source = "<?php
function dead(): void {
    if (false) { throw new DomainException(); }
    if (true) {} elseif (true) { throw new OutOfBoundsException(); } else { throw new BadMethodCallException(); }
    while (false) { throw new LengthException(); }
    false ? throw new RuntimeException() : null;
    true ? null : throw new UnderflowException();
    try {} catch (Throwable $e) { throw new OverflowException(); }
}
function live(): void { do { throw new DomainException(); } while (false); }
";
    assert!(missing_for(source, "dead").is_empty());
    assert!(missing_for(source, "live")[0].contains("DomainException"));
}

#[test]
fn throwing_match_arms_propagate_to_callers_and_can_be_caught() {
    let source = "<?php
function leaf(bool $raise): int { return match ($raise) { true => throw new DomainException(), false => 1 }; }
function caller(): void { leaf(true); }
function caught(): void { try { leaf(true); } catch (DomainException) {} }
";
    assert!(missing_for(source, "caller")[0].contains("DomainException"));
    assert!(missing_for(source, "caught").is_empty());
}

#[test]
fn callback_effects_depend_on_literal_arguments() {
    let source = "<?php
/** @param callable(): void $callback */
function invoke(callable $callback, bool $enabled): void { if ($enabled) { $callback(); } }
function no(): void { invoke(static function(): void { throw new DomainException(); }, false); }
function yes(): void { invoke(static function(): void { throw new DomainException(); }, true); }
";
    assert!(missing_for(source, "no").is_empty());
    assert!(missing_for(source, "yes")[0].contains("DomainException"));
}

#[test]
fn recursion_converges_and_reassigned_catch_variables_use_the_new_type() {
    let source = "<?php
function recursive(bool $stop): void { if ($stop) { throw new DomainException(); } recursive(true); }
function caller(): void { recursive(false); }
function replacement(): void { try { caller(); } catch (Throwable $e) { $e = new LengthException(); throw $e; } }
";
    assert!(missing_for(source, "caller")[0].contains("DomainException"));
    let replacement = missing_for(source, "replacement");
    assert_eq!(replacement.len(), 1);
    assert!(replacement[0].contains("LengthException"));
    assert!(!replacement[0].contains("DomainException"));
}

#[test]
fn local_closure_value_captures_follow_the_specialized_parent() {
    for source in [
        "<?php
function leaf(bool $enabled): void { $callback = static fn() => $enabled ? throw new DomainException() : null; $callback(); }
function no(): void { leaf(false); }
function yes(): void { leaf(true); }
",
        "<?php
function leaf(bool $enabled): void {
    $read = static fn() => $enabled;
    $captured = $read();
    $callback = static function() use ($captured): void { if ($captured) { throw new DomainException(); } };
    $callback();
}
function no(): void { leaf(false); }
function yes(): void { leaf(true); }
",
    ] {
        assert!(missing_for(source, "no").is_empty());
        assert!(missing_for(source, "yes")[0].contains("DomainException"));
    }
}

#[test]
fn array_callable_arguments_keep_their_target() {
    let source = "<?php
final class Handler { public static function run(): void { throw new DomainException(); } }
/** @param callable(): void $callback */
function invoke(callable $callback): void { $callback(); }
function caller(): void { invoke([Handler::class, 'run']); }
";
    assert!(missing_for(source, "caller")[0].contains("DomainException"));
}

#[test]
fn thrown_parameter_uses_its_actual_class_and_intersections_exclude_other_interfaces() {
    let source = "<?php
function leaf(Throwable $error): void { throw $error; }
function caller(): void { leaf(new DomainException()); }
function intersection(Exception&Countable $error): void { throw $error; }
/**
 * @template T of Exception
 * @param T $error
 */
function generic(Exception $error): void { throw $error; }
";
    let caller = missing_for(source, "caller");
    assert_eq!(caller.len(), 1);
    assert!(caller[0].contains("DomainException"));
    assert!(!caller[0].contains("`Throwable`"));
    let intersection = missing_for(source, "intersection");
    assert_eq!(intersection.len(), 1);
    assert!(intersection[0].contains("Exception"));
    assert!(!intersection[0].contains("Countable"));
    assert!(missing_for(source, "generic")[0].contains("Exception"));
    let assertion = "<?php
/**
 * @template T
 * @param class-string<T> $class
 */
function ensure(object $value, string $class): void {
    if (!$value instanceof $class) { throw new DomainException(); }
}
";
    assert!(missing_for(assertion, "ensure")[0].contains("DomainException"));
}

#[test]
fn unknown_try_effects_keep_handlers_reachable_and_rethrows_broad() {
    let source = "<?php
/** @param callable(): void $callback */
function handler(callable $callback): void { try { $callback(); } catch (Throwable) { throw new DomainException(); } }
/** @param callable(): void $callback */
function rethrow(callable $callback): void { try { if (random_int(0, 1)) { throw new DomainException(); } $callback(); } catch (Throwable $error) { throw $error; } }
";
    assert!(missing_for(source, "handler")[0].contains("DomainException"));
    assert!(missing_for(source, "rethrow").iter().any(|message| message.contains("`Throwable`")));
}

#[test]
fn array_map_runs_for_nonempty_remaining_arrays() {
    let source = "<?php
function multiple(): void { array_map(static function (?int $left, int $right): int { throw new DomainException(); }, [], [1]); }
function empty(): void { array_map(static function (?int $left, ?int $right): int { throw new DomainException(); }, [], []); }
function named(): void { array_map(array: [], callback: static function (int $value): int { throw new DomainException(); }); }
";
    assert!(missing_for(source, "multiple")[0].contains("DomainException"));
    assert!(missing_for(source, "empty").is_empty());
    assert!(missing_for(source, "named").is_empty());
}

#[test]
fn unresolved_method_effects_preserve_existing_contracts() {
    let source = "<?php
/** @throws RuntimeException */
function unresolved(object $service): void { $service->send(); }
";
    let issues = analyze(source);
    assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("throws-inference-incomplete")));
    assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some("unused-throws-type")));
    assert_eq!(fix(source), source);
    let abstract_method = "<?php
interface Service { public function send(): void; }
/** @throws RuntimeException */
function unresolved(Service $service): void { $service->send(); }
";
    let issues = analyze(abstract_method);
    assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("throws-inference-incomplete")));
    assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some("unused-throws-type")));
    assert_eq!(fix(abstract_method), abstract_method);
    let nullsafe = "<?php
/** @throws RuntimeException */
function nullsafe(): void { $service = null; $service?->send(); }
";
    let issues = analyze(nullsafe);
    assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("unused-throws-type")));
    assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some("throws-inference-incomplete")));
}
