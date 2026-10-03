#!/usr/bin/env python3
"""Exercise native CLI inference, safe fixing, cache invalidation and Git selection on temporary PHP fixtures."""

import argparse
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def command(args, cwd, **kwargs):
    return subprocess.run(args, cwd=cwd, check=True, capture_output=True, **kwargs)


def analyze(binary, workspace, *args):
    invocation = [
        'prlimit', '--as=1073741824', '--cpu=45',
        'timeout', '--signal=TERM', '--kill-after=5s', '40s',
        binary, '--workspace', str(workspace), '--config', str(workspace / 'mago.toml'),
        '--threads', '1', '--colors', 'never', 'analyze', '--no-extensions',
        '--throws-only', *([] if '--fix' in args else ['--reporting-format', 'json']), *args,
    ]
    result = subprocess.run(invocation, cwd=workspace, capture_output=True, timeout=50)
    assert result.returncode in (0, 1), result.stderr.decode()
    return [] if '--fix' in args else json.loads(result.stdout)['issues']


def signature(issues):
    return sorted((issue['code'], issue['message'], tuple(sorted((a['span']['file_id']['name'], a['span']['start']['offset']) for a in issue['annotations'] if a['kind'] == 'Primary'))) for issue in issues)


def missing(issues, function):
    return [issue for issue in issues if issue['code'] == 'unhandled-thrown-type' and f'`{function}`' in issue['message']]


def write_config(workspace):
    (workspace / 'mago.toml').write_text('php-version = "8.5.0"\n[source]\npaths = ["cases"]\nincludes = ["dependencies"]\n[analyzer]\ncheck-throws = true\nfind-unused-definitions = false\nfind-unused-expressions = false\n')


def git_base(workspace):
    # Create an isolated test snapshot without commits in the fork checkout.
    command(['git', 'init', '--quiet'], workspace)
    command(['git', 'add', 'cases', 'dependencies', 'mago.toml'], workspace)
    tree = command(['git', 'write-tree'], workspace).stdout.strip()
    identity = dict(os.environ, GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid')
    return command(['git', 'commit-tree', tree.decode()], workspace, input=b'PHP fixture baseline\n', env=identity).stdout.decode().strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mago', required=True)
    args = parser.parse_args()
    binary = str(Path(args.mago).resolve())
    corpus = Path(__file__).resolve().parent
    # Check disputed callback effects against PHP execution, independently of both analyzers.
    runtime = command([
        'prlimit', '--as=1073741824', '--cpu=10', 'timeout', '15s',
        'php', '-d', 'memory_limit=128M',
    ], corpus, input=b'''<?php
require 'dependencies/callbacks.php';
require 'cases/immediate_closure.php';
require 'cases/native_callback.php';
require 'cases/contract_callback.php';
require 'cases/conditional.php';
$calls = [
    'immediate' => 'ThrowsProbe\\\\immediate_closure\\\\caller',
    'native' => 'ThrowsProbe\\\\native_callback\\\\caller',
    'empty' => 'ThrowsProbe\\\\native_callback\\\\emptyArray',
    'multiple' => 'ThrowsProbe\\\\native_callback\\\\multipleArrays',
    'all-empty' => 'ThrowsProbe\\\\native_callback\\\\emptyArrays',
    'contract' => 'ThrowsProbe\\\\contract_callback\\\\caller',
    'false' => 'ThrowsProbe\\\\conditional\\\\caller_false',
    'true' => 'ThrowsProbe\\\\conditional\\\\caller_true',
];
$effects = [];
foreach ($calls as $name => $call) {
    try { $call(); $effects[$name] = null; }
    catch (Throwable $error) { $effects[$name] = get_class($error); }
}
echo json_encode($effects, JSON_THROW_ON_ERROR);
''', timeout=20)
    assert json.loads(runtime.stdout) == {
        'immediate': 'DomainException', 'native': 'DomainException', 'empty': None,
        'multiple': 'DomainException', 'all-empty': None,
        'contract': 'DomainException', 'false': None, 'true': 'DomainException',
    }
    print('PHP runtime callback and conditional effects: passed', flush=True)
    with tempfile.TemporaryDirectory(prefix='mago-native-throws-test-') as temp:
        workspace = Path(temp)
        shutil.copytree(corpus / 'cases', workspace / 'cases')
        shutil.copytree(corpus / 'dependencies', workspace / 'dependencies')
        write_config(workspace)
        issues = analyze(binary, workspace)
        assert missing(issues, 'ThrowsProbe\\cross_file\\caller')
        assert not missing(issues, 'ThrowsProbe\\conditional\\caller_false')
        assert missing(issues, 'ThrowsProbe\\conditional\\caller_true')
        assert 'LengthException' in missing(issues, 'ThrowsProbe\\finally_override\\caller')[0]['message']
        assert 'DomainException' in missing(issues, 'ThrowsProbe\\rethrow\\caller')[0]['message']
        assert missing(issues, 'ThrowsProbe\\synchronous_callback\\caller')
        assert missing(issues, 'ThrowsProbe\\immediate_closure\\caller')
        assert missing(issues, 'ThrowsProbe\\native_callback\\caller')
        assert missing(issues, 'ThrowsProbe\\native_callback\\multipleArrays')
        assert missing(issues, 'ThrowsProbe\\contract_callback\\caller')
        assert not missing(issues, 'ThrowsProbe\\native_callback\\emptyArray')
        assert not missing(issues, 'ThrowsProbe\\native_callback\\emptyArrays')
        assert not missing(issues, 'ThrowsProbe\\native_callback\\deferred')
        assert not missing(issues, 'ThrowsProbe\\contract_callback\\deferred')
        assert any(issue['code'] == 'overly-wide-throws-type' for issue in issues)
        assert any(issue['code'] == 'unused-throws-type' for issue in issues)
        assert all(issue['code'] in {'unhandled-thrown-type', 'overly-wide-throws-type', 'unused-throws-type', 'throws-inference-incomplete'} for issue in issues)
        assert signature(analyze(binary, workspace, '--throws-cache', 'state.json')) == signature(issues)
        assert (workspace / 'state.json').is_file()
        assert signature(analyze(binary, workspace, '--throws-cache', 'state.json')) == signature(issues)

        leaf = workspace / 'cases/cross_file_leaf.php'
        stat = leaf.stat()
        leaf.write_text(leaf.read_text().replace('DomainException', 'LengthException'))
        os.utime(leaf, ns=(stat.st_atime_ns, stat.st_mtime_ns))
        changed = analyze(binary, workspace, '--throws-cache', 'state.json')
        assert signature(changed) == signature(analyze(binary, workspace))
        assert 'LengthException' in missing(changed, 'ThrowsProbe\\cross_file\\caller')[0]['message']
        leaf.write_text(leaf.read_text().replace('throw new \\LengthException();', ''))
        removed = analyze(binary, workspace, '--throws-cache', 'state.json')
        assert signature(removed) == signature(analyze(binary, workspace))
        assert not missing(removed, 'ThrowsProbe\\cross_file\\caller')
        external = workspace / 'dependencies/external.php'
        external.write_text(external.read_text().replace('LengthException', 'DomainException'))
        changed_external = analyze(binary, workspace, '--throws-cache', 'state.json')
        assert signature(changed_external) == signature(analyze(binary, workspace))
        assert 'DomainException' in missing(changed_external, 'ThrowsProbe\\external\\caller')[0]['message']
        (workspace / 'state.json').write_text('{corrupted')
        assert signature(analyze(binary, workspace, '--throws-cache', 'state.json')) == signature(changed_external)
        analyze(binary, workspace, '--throws-explain', 'origins.json')
        explained = json.loads((workspace / 'origins.json').read_text())
        caller = next(function for function in explained['functions'] if function['function'] == 'throwsprobe\\cross_file\\caller')
        assert not caller['summary']['exceptions']
        chain = next(function for function in explained['functions'] if function['function'] == 'throwsprobe\\chain_inferred\\caller')
        assert chain['summary']['exceptions'][0]['origins'][0]['callee']
        print('CLI inference and content/dependency cache invalidation: passed', flush=True)

        scope = workspace / 'cases/selection.php'
        scope.write_text('<?php\nnamespace Selection;\nfunction changed(): void { helper(); }\nfunction untouched(): void { throw new \\RuntimeException(); }\nfunction helper(): void { throw new \\DomainException(); }\n')
        base = git_base(workspace)
        scope.write_text(scope.read_text().replace('helper();', 'helper(); /* changed */'))
        selected = analyze(binary, workspace, '--throws-diff', base)
        assert len(selected) == 1, signature(selected)
        assert missing(selected, 'Selection\\changed')
        before_helper = scope.read_text().split('function untouched()', 1)[1]
        analyze(binary, workspace, '--throws-diff', base, '--fix', '--potentially-unsafe')
        fixed = scope.read_text()
        assert '@throws \\DomainException' in fixed
        assert fixed.split('function untouched()', 1)[1] == before_helper
        command(['php', '-l', str(scope)], workspace)
        assert not analyze(binary, workspace, '--throws-diff', base)
        assert len(analyze(binary, workspace, '--throws-diff', base, '--throws-full-file', 'cases/selection.php')) == 2
        scope.write_text('<?php function broken( {')
        assert any(issue['code'] == 'parse' for issue in analyze(binary, workspace, '--throws-diff', base))
        print('Git declaration selection and PHPDoc fixing: passed', flush=True)

    with tempfile.TemporaryDirectory(prefix='mago-native-throws-fix-') as temp:
        workspace = Path(temp)
        shutil.copytree(corpus / 'cases', workspace / 'cases')
        shutil.copytree(corpus / 'dependencies', workspace / 'dependencies')
        write_config(workspace)
        external_before = (workspace / 'dependencies/external.php').read_bytes()
        analyze(binary, workspace, '--fix', '--potentially-unsafe')
        for file in (workspace / 'cases').glob('*.php'):
            command(['php', '-l', str(file)], workspace)
        remaining = analyze(binary, workspace)
        assert not remaining, signature(remaining)
        hashes = {file: file.read_bytes() for file in (workspace / 'cases').glob('*.php')}
        analyze(binary, workspace, '--fix', '--potentially-unsafe')
        assert all(file.read_bytes() == content for file, content in hashes.items())
        assert (workspace / 'dependencies/external.php').read_bytes() == external_before
        print('Whole corpus fixing, PHP syntax and idempotence: passed', flush=True)

    with tempfile.TemporaryDirectory(prefix='mago-native-throws-unresolved-') as temp:
        workspace = Path(temp)
        (workspace / 'cases').mkdir()
        (workspace / 'dependencies').mkdir()
        write_config(workspace)
        file = workspace / 'cases/unresolved.php'
        source = '<?php\n/** @throws RuntimeException */\nfunction run(object $service): void { $service->send(); }\n'
        file.write_text(source)
        issues = analyze(binary, workspace)
        assert any(issue['code'] == 'throws-inference-incomplete' for issue in issues)
        assert not any(issue['code'] == 'unused-throws-type' for issue in issues)
        analyze(binary, workspace, '--fix', '--potentially-unsafe')
        assert file.read_text() == source
        print('Unresolved method contracts remain intact after CLI fixing: passed', flush=True)

    with tempfile.TemporaryDirectory(prefix='mago-native-yii-test-') as temp:
        workspace = Path(temp)
        shutil.copytree(corpus / 'yii2/cases', workspace / 'cases')
        shutil.copytree(corpus / 'yii2/dependencies', workspace / 'dependencies')
        write_config(workspace)
        config = workspace / 'mago.toml'
        config.write_text(config.read_text() + 'plugins = ["yii2"]\n')
        for file in workspace.rglob('*.php'):
            command(['php', '-l', str(file)], workspace)
        issues = analyze(binary, workspace, '--throws-cache', 'state.json')
        for function, exception in {'validate': 'DomainException', 'saveTrue': 'DomainException', 'saveHook': 'LengthException', 'event': 'UnexpectedValueException', 'construct': 'OverflowException', 'create': 'OverflowException', 'createConfig': 'OverflowException', 'setter': 'UnderflowException', 'ruleCallback': 'RangeException', 'query': 'RuntimeException'}.items():
            found = missing(issues, 'YiiProbe\\' + function)
            assert found and exception in found[0]['message'], (function, signature(issues))
        for function in ['saveFalse', 'saveNamedFalse', 'registerOnly', 'safeSetter', 'realProperty']:
            assert not missing(issues, 'YiiProbe\\' + function), (function, signature(issues))
        assert signature(analyze(binary, workspace, '--throws-cache', 'state.json')) == signature(issues)
        hooks = workspace / 'cases/hooks.php'
        hooks.write_text(hooks.read_text().replace('DomainException', 'LengthException'))
        changed = analyze(binary, workspace, '--throws-cache', 'state.json')
        assert signature(changed) == signature(analyze(binary, workspace))
        assert 'LengthException' in missing(changed, 'YiiProbe\\saveTrue')[0]['message']
        analyze(binary, workspace, '--fix', '--potentially-unsafe')
        assert not analyze(binary, workspace)
        print('Yii lifecycle, validators, events, factory, setters and query contracts: passed', flush=True)


if __name__ == '__main__':
    main()
