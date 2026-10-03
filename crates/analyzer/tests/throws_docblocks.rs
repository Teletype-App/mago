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
    analyze_with_settings(source, Settings { check_throws: true, find_unused_parameters: false, ..Settings::default() })
}

fn analyze_with_settings(source: &str, settings: Settings) -> IssueCollection {
    let Prelude { mut metadata, mut symbol_references, .. } = PRELUDE.clone();
    let file = File::ephemeral(Cow::Borrowed(b"contract.php"), Cow::Owned(source.as_bytes().to_vec()));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    assert!(!program.has_errors(), "Invalid PHP: {:?}", program.errors);
    let names = NameResolver::new(&arena).resolve(program);
    metadata.extend(scan_program(&arena, &file, program, &names, settings.version));
    populate_codebase(&mut metadata, &mut symbol_references, WordSet::default(), HashSet::default());
    let registry = PluginRegistry::with_library_providers();
    let mut result = AnalysisResult::new(symbol_references);
    Analyzer::new(&arena, &file, &names, &metadata, &registry, settings).analyze(program, &mut result).unwrap();
    result.issues.extend(metadata.take_issues(true));
    result.issues
}

#[test]
fn generator_effects_start_on_consumption_and_survive_factory_returns() {
    let source = "<?php
/** @param callable(): void $callback */
function stream(callable $callback): Generator { yield $callback(); }
function deferred(): Generator { return stream(static function(): void { throw new DomainException(); }); }
function consume(): void { foreach (deferred() as $value) {} }
function consumeVariable(): void { $values = deferred(); foreach ($values as $value) {} }
function consumeNative(): void { iterator_to_array(deferred()); }
function consumeMethod(): void { $values = deferred(); $values->rewind(); }
function relay(): Generator { yield from deferred(); }
function consumeRelay(): void { foreach (relay() as $value) {} }
function caught(): void { try { foreach (deferred() as $value) {} } catch (DomainException) {} }
function discarded(): void { $values = deferred(); $values = []; foreach ($values as $value) {} }
";
    assert!(missing_for(source, "deferred").is_empty());
    for function in ["consume", "consumeVariable", "consumeNative", "consumeMethod", "consumeRelay"] {
        assert!(missing_for(source, function).iter().any(|message| message.contains("DomainException")), "{function}");
    }
    for function in ["caught", "discarded"] {
        assert!(missing_for(source, function).is_empty(), "{function}");
    }
    let unknown =
        "<?php /** @throws RuntimeException */ function consume(Generator $values): void { $values->next(); }";
    assert!(analyze(unknown).iter().any(|issue| issue.code.as_deref() == Some("throws-inference-incomplete")));
    assert_eq!(fix(unknown), unknown);
}

#[test]
fn ignored_exception_contracts_respect_exact_classes_and_hierarchies() {
    use mago_word::word;
    let source = "<?php /** @throws RuntimeException */ function run(): void {}";
    for (name, descendants, unused) in [
        ("RuntimeException", false, false),
        ("rUnTiMeExCePtIoN", false, false),
        ("Exception", true, false),
        ("Exception", false, true),
        ("LogicException", true, true),
    ] {
        let mut settings = Settings { check_throws: true, ..Settings::default() };
        if descendants {
            settings.unchecked_exceptions.insert(word(name));
        } else {
            settings.unchecked_exception_classes.insert(word(name));
        }
        let issues = analyze_with_settings(source, settings);
        assert_eq!(issues.iter().any(|issue| issue.code.as_deref() == Some("unused-throws-type")), unused, "{name}");
    }
}

#[test]
fn global_exceptions_are_reported_independently_of_function_docblocks() {
    let settings = Settings { check_throws_in_global_scope: true, ..Settings::default() };
    for (name, source, exceptions) in [
        ("direct", "<?php throw new DomainException();", vec!["DomainException"]),
        (
            "inferred call",
            "<?php function leaf(): void { throw new DomainException(); } leaf();",
            vec!["DomainException"],
        ),
        (
            "namespace",
            "<?php namespace App; function leaf(): void { throw new \\DomainException(); } leaf();",
            vec!["DomainException"],
        ),
        (
            "caught",
            "<?php function leaf(): void { throw new DomainException(); } try { leaf(); } catch (Throwable) {}",
            vec![],
        ),
        (
            "finally",
            "<?php try { throw new DomainException(); } finally { throw new LengthException(); }",
            vec!["LengthException"],
        ),
        (
            "suppressed",
            "<?php\n/** @mago-ignore analysis:uncaught-throw-in-global-scope */\nthrow new DomainException();",
            vec![],
        ),
    ] {
        let issues = analyze_with_settings(source, settings.clone());
        let global = issues
            .iter()
            .filter(|issue| issue.code.as_deref() == Some("uncaught-throw-in-global-scope"))
            .collect::<Vec<_>>();
        assert_eq!(global.len(), exceptions.len(), "{name}: {issues:?}");
        for exception in exceptions {
            assert!(global.iter().any(|issue| issue.message.contains(exception)), "{name}");
        }
        assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some("unhandled-thrown-type")), "{name}");
        assert!(global.iter().all(|issue| issue.edits.is_empty()), "Top-level code has no function PHPDoc to fix");
    }
}

#[test]
fn global_exception_ignores_do_not_hide_function_contract_errors() {
    use mago_word::word;
    let source = "<?php function leaf(): void { throw new DomainException(); } leaf();";
    for (name, descendants, hidden) in
        [("dOmAiNeXcEpTiOn", false, true), ("LogicException", true, true), ("LogicException", false, false)]
    {
        let mut settings = Settings { check_throws: true, check_throws_in_global_scope: true, ..Settings::default() };
        if descendants {
            settings.unchecked_exceptions_in_global_scope.insert(word(name));
        } else {
            settings.unchecked_exception_classes_in_global_scope.insert(word(name));
        }
        let issues = analyze_with_settings(source, settings);
        assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("unhandled-thrown-type")), "{name}");
        assert_eq!(
            issues.iter().any(|issue| issue.code.as_deref() == Some("uncaught-throw-in-global-scope")),
            !hidden,
            "{name}"
        );
    }
}

#[test]
fn exception_factories_fluent_construction_and_native_callables_keep_their_effects() {
    let cases = [
        (
            "factory",
            "function make(): DomainException { return new DomainException(); } function run(): void { throw make(); }",
            "DomainException",
        ),
        (
            "static factory",
            "final class Factory { public static function make(): DomainException { return new DomainException(); } } function run(): void { throw Factory::make(); }",
            "DomainException",
        ),
        (
            "fluent constructor",
            "class Fault extends DomainException { public function context(): self { return $this; } } function run(): void { throw (new Fault())->context(); }",
            "Fault",
        ),
        ("native random", "function run(): int { return random_int(1, 2); }", "Random\\RandomException"),
        ("first class native", "function run(): Closure { return random_int(...); }", ""),
        (
            "ordered catch",
            "function run(): void { try { throw new DomainException(); } catch (DomainException) {} catch (Throwable $e) { throw new LengthException(); } }",
            "",
        ),
    ];
    for (name, body, exception) in cases {
        let source = format!("<?php {body}");
        let missing = missing_for(&source, "run");
        if exception.is_empty() {
            assert!(missing.is_empty(), "{name}: {missing:?}");
        } else {
            assert_eq!(missing.len(), 1, "{name}: {missing:?}");
            assert!(missing[0].contains(exception), "{name}: {missing:?}");
        }
    }
}

#[test]
fn integer_division_reports_only_reachable_native_errors() {
    for (source, expected) in [
        ("function run(int $value): int { return intdiv($value, 1); }", vec![]),
        ("function run(int $value): int { return intdiv($value, 0); }", vec!["DivisionByZeroError"]),
        ("function run(): int { return intdiv(PHP_INT_MIN, -1); }", vec!["ArithmeticError"]),
        ("function run(): int { return intdiv(1, -1); }", vec![]),
        (
            "/** @throws RangeException */ function run(int $value, int $divisor): int { if ($divisor <= 0) { throw new RangeException(); } return intdiv($value, $divisor); }",
            vec![],
        ),
        (
            "function run(int $value, int $divisor): int { return intdiv($value, $divisor); }",
            vec!["ArithmeticError", "DivisionByZeroError"],
        ),
    ] {
        let missing = missing_for(&format!("<?php {source}"), "run");
        assert_eq!(missing.len(), expected.len(), "{source}: {missing:?}");
        for exception in expected {
            assert!(missing.iter().any(|issue| issue.contains(&format!("`{exception}`"))), "{source}");
        }
    }
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
fn explicit_inheritdoc_extends_the_parent_exception_contract() {
    for inherited in ["{@inheritdoc}", "@inheritdoc"] {
        let source = format!(
            "<?php
interface Contract {{ /** @throws DomainException */ public function run(bool $parent): void; }}
final class Worker implements Contract {{
    /** {inherited}
     * @throws LengthException
     */
    public function run(bool $parent): void {{
        if ($parent) {{ throw new DomainException(); }}
        throw new LengthException();
    }}
}}"
        );
        assert!(missing_for(&source, "Worker::run").is_empty(), "{inherited}: {:?}", analyze(&source));
        assert_eq!(fix(&source), source);
        let overridden = source.replace(inherited, "Own contract.");
        assert!(missing_for(&overridden, "Worker::run")[0].contains("DomainException"));
    }
}

#[test]
fn yii_hooks_work_with_their_declared_framework_roots() {
    for (root, method, hook) in [
        ("yii\\base\\Model", "validate", "afterValidate"),
        ("yii\\db\\BaseActiveRecord", "save", "afterSave"),
        ("yii\\base\\Component", "trigger", "handle"),
    ] {
        let (namespace, class) = root.rsplit_once('\\').unwrap();
        let source = if method == "trigger" {
            format!("<?php
namespace {namespace} {{ class {class} {{ public function on(string $name, callable $handler): void {{}} public function trigger(string $name): void {{}} }} }}
namespace App {{ final class Worker extends \\{root} {{
    public function init(): void {{ $this->on('run', [$this, 'handle']); }}
    /** @throws \\DomainException */ public function handle(): void {{ throw new \\DomainException(); }}
}}
function run(Worker $worker): void {{ $worker->trigger('run'); }}
function register(Worker $worker): void {{ $worker->on('other', [$worker, 'handle']); }} }}")
        } else {
            format!("<?php
namespace {namespace} {{ class {class} {{ public function {method}(): void {{}} public function {hook}(): void {{}} }} }}
namespace App {{ final class Worker extends \\{root} {{
    /** @throws \\DomainException */ public function {hook}(): void {{ throw new \\DomainException(); }}
}}
function run(Worker $worker): void {{ $worker->{method}(); }} }}")
        };
        assert!(missing_for(&source, "App\\run").iter().any(|issue| issue.contains("DomainException")), "{root}");
        assert!(missing_for(&source, "App\\register").is_empty());
    }
}

#[test]
fn yii_magic_property_hooks_do_not_replace_real_properties() {
    let source = "<?php
namespace yii\\base { class Component { public function __get(string $name): mixed { return null; } public function __set(string $name, mixed $value): void {} } }
namespace App {
final class Worker extends \\yii\\base\\Component {
    public string $plain = '';
    /** @throws \\DomainException */ public function getLabel(): string { throw new \\DomainException(); }
    /** @throws \\LengthException */ public function setLabel(string $value): void { throw new \\LengthException(); }
    /** @throws \\OverflowException */ public function getPlain(): string { throw new \\OverflowException(); }
    /** @throws \\UnderflowException */ public function setPlain(string $value): void { throw new \\UnderflowException(); }
}
function read(Worker $worker): void { echo $worker->label; }
function write(Worker $worker): void { $worker->label = 'new'; }
function real(Worker $worker): void { echo $worker->plain; $worker->plain = 'new'; }
}
";
    assert!(missing_for(source, "App\\read").iter().any(|issue| issue.contains("DomainException")));
    assert!(missing_for(source, "App\\write").iter().any(|issue| issue.contains("LengthException")));
    assert!(missing_for(source, "App\\real").is_empty());
}

#[test]
fn shared_trait_parent_calls_use_each_consumers_parent() {
    let source = "<?php
class DomainBase { /** @throws DomainException */ public function save(): void { throw new DomainException(); } }
class LengthBase { /** @throws LengthException */ public function save(): void { throw new LengthException(); } }
trait Save { public function save(): void { parent::save(); } }
final class DomainWorker extends DomainBase { use Save; }
final class LengthWorker extends LengthBase { use Save; }
function domain(DomainWorker $worker): void { $worker->save(); }
function length(LengthWorker $worker): void { $worker->save(); }
";
    for (function, expected, excluded) in
        [("domain", "DomainException", "LengthException"), ("length", "LengthException", "DomainException")]
    {
        let missing = missing_for(source, function);
        assert_eq!(missing.len(), 1, "{function}: {missing:?}");
        assert!(missing[0].contains(expected), "{function}: {missing:?}");
        assert!(!missing[0].contains(excluded), "{function}: {missing:?}");
    }
}

#[test]
fn invalid_exception_contracts_are_reported_and_not_erased() {
    for source in [
        "<?php /** @throws MissingException */ function run(): void {}",
        "<?php class NotAnException {} /** @throws NotAnException */ function run(): void {}",
        "<?php /** @throws int */ function run(): void {}",
    ] {
        let issues = analyze(source);
        assert!(issues.iter().any(|issue| issue.code.as_deref() == Some("invalid-throws-type")), "{issues:?}");
        assert_eq!(fix(source), source);
    }
}

#[test]
fn malformed_exception_tags_are_preserved_by_the_fixer() {
    let source =
        "<?php class Worker { /** @throws Exception*@throws RuntimeException */ public function run(): void {} }";
    assert_eq!(fix(source), source);
}

#[test]
fn suppressed_docblock_changes_do_not_escape_through_other_diagnostics() {
    for (ignored, documented, body) in [
        ("unused-throws-type", "RuntimeException", "throw new DomainException();"),
        ("overly-wide-throws-type", "Exception", "if ($raise) { throw new DomainException(); } throw new TypeError();"),
        ("unhandled-thrown-type", "RuntimeException", "throw new DomainException();"),
    ] {
        let source = format!(
            "<?php\n/**\n * @throws {documented}\n * @mago-ignore analysis:{ignored}\n */\nfunction run(bool $raise): void {{ {body} }}\n"
        );
        let issues = analyze(&source);
        assert!(!issues.iter().any(|issue| issue.code.as_deref() == Some(ignored)), "{ignored}");
        assert!(
            issues.iter().any(|issue| matches!(
                issue.code.as_deref(),
                Some("unhandled-thrown-type" | "unused-throws-type" | "overly-wide-throws-type")
            )),
            "The other diagnosis still applies: {issues:?}"
        );
        assert_eq!(fix(&source), source, "{ignored}");
    }
}

#[test]
fn duplicate_exception_tags_are_removed_without_losing_descriptions() {
    for source in [
        "<?php\n/**\n * @throws DomainException\n * @throws DomainException When the value is invalid.\n */\nfunction run(): void { throw new DomainException(); }\n",
        "<?php\n/**\n * @throws DomainException|LengthException When the value is invalid.\n * @throws DomainException\n */\nfunction run(bool $domain): void { if ($domain) { throw new DomainException(); } throw new LengthException(); }\n",
    ] {
        let fixed = fix(source);
        assert_eq!(fixed.matches("@throws").count(), 1, "{fixed}");
        assert!(fixed.contains("When the value is invalid."));
        assert!(analyze(&fixed).iter().all(|issue| issue.code.as_deref() != Some("unhandled-thrown-type")));
        assert_eq!(fix(&fixed), fixed);
    }
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

#[test]
fn removing_a_throw_from_a_recursive_context_matches_fresh_inference() {
    use mago_analyzer::throws::ThrowsSummaries;
    use mago_codex::identifier::function_like::FunctionLikeIdentifier;
    use mago_syntax::settings::ParserSettings;
    use mago_word::word;

    let source = "<?php
function route(bool $execute = false): void {
    if ($execute) { run(); } else { throw new DomainException(); }
}
function run(?bool $queue = null): void { route($queue === null); THROW }
function caller(): void { run(); }
";
    let settings = Settings { check_throws: true, find_unused_parameters: false, ..Settings::default() };
    let registry = PluginRegistry::with_library_providers();
    let mut seed = ThrowsSummaries::default();
    for throwing in [true, false] {
        let text = source.replace("THROW", if throwing { "throw new LengthException();" } else { "" });
        let file = File::ephemeral(Cow::Borrowed(b"contract.php"), Cow::Owned(text.into_bytes()));
        let Prelude { mut metadata, mut symbol_references, .. } = PRELUDE.clone();
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        assert!(!program.has_errors());
        let names = NameResolver::new(&arena).resolve(program);
        metadata.extend(scan_program(&arena, &file, program, &names, settings.version));
        populate_codebase(&mut metadata, &mut symbol_references, WordSet::default(), HashSet::default());
        seed = ThrowsSummaries::infer_incremental(
            &[&file],
            &metadata,
            &registry,
            &settings,
            ParserSettings::default(),
            seed,
            Some(&HashSet::from_iter([file.id])),
        )
        .unwrap();
        let caller = &seed.functions[&FunctionLikeIdentifier::Function(word("caller"))];
        if throwing {
            assert!(caller.exceptions.contains_key(&word("LengthException")));
        } else {
            assert!(caller.exceptions.is_empty(), "Removed throws must not circulate through recursion: {caller:?}");
            let fresh =
                ThrowsSummaries::infer(&[&file], &metadata, &registry, &settings, ParserSettings::default()).unwrap();
            assert_eq!(caller, &fresh.functions[&FunctionLikeIdentifier::Function(word("caller"))]);
            assert!(
                seed.functions[&FunctionLikeIdentifier::Function(word("route"))]
                    .exceptions
                    .contains_key(&word("DomainException"))
            );
        }
    }
}
