#!/usr/bin/env python3
"""Compare analyzer CLI results on the checked-in PHP corpus with bounded resources."""

import argparse
import hashlib
import html
import json
import shutil
import statistics
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path


def run_analyzer(name, command, run_dir, output_dir, iteration):
    prefix = output_dir / f'{name}-{iteration}'
    metrics = prefix.with_suffix('.metrics')
    bounded = [
        '/usr/bin/time', '-f', '%e\t%M\t%x', '-o', str(metrics),
        'prlimit', '--as=1073741824', '--cpu=45', '--nofile=4096',
        'timeout', '--signal=TERM', '--kill-after=5s', '40s', *command,
    ]
    with prefix.with_suffix('.json').open('wb') as out, prefix.with_suffix('.stderr').open('wb') as err:
        result = subprocess.run(bounded, cwd=run_dir, stdout=out, stderr=err, timeout=50)
    try:
        issues = json.loads(prefix.with_suffix('.json').read_text())
        if name == 'mago':
            issues = issues['issues']
        assert isinstance(issues, list)
        # Exit 1 (Mago) and 2 (Psalm) mean diagnostics, not process failure.
        assert result.returncode in ({0, 1} if name == 'mago' else {0, 2})
        fields = metrics.read_text().splitlines()[-1].split('\t')
        elapsed, rss, code = float(fields[0]), int(fields[1]), int(fields[2])
        assert code == result.returncode
    except (ValueError, KeyError, AssertionError) as error:
        raise RuntimeError(f'{name} did not complete analysis. See {prefix}.stderr') from error
    print(f'{name} run {iteration}: {elapsed:.2f}s, {rss / 1024:.1f} MiB', flush=True)
    return {'elapsed_seconds': elapsed, 'max_rss_kib': rss, 'exit_code': code, 'issues': issues}


def throws_by_file(name, issues):
    result = {}
    for issue in issues:
        code = issue.get('code') if name == 'mago' else issue['type']
        if code not in {'unhandled-thrown-type', 'overly-wide-throws-type', 'unused-throws-type', 'throws-inference-incomplete', 'MissingThrowsDocblock', 'OverlyBroadThrowsDocblock', 'UnusedThrowsDocblock'}:
            continue
        if name == 'mago':
            primary = next(a for a in issue['annotations'] if a['kind'] == 'Primary')
            file = Path(primary['span']['file_id']['name']).name
            line = primary['span']['start']['line'] + 1
        else:
            file = Path(issue['file_name']).name
            line = issue['line_from']
        result.setdefault(file, []).append({'code': code, 'line': line, 'message': issue['message']})
    for entries in result.values():
        entries.sort(key=lambda item: (item['line'], item['code'], item['message']))
    return dict(sorted(result.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mago', default='mago', help='Mago executable, optionally a fork build')
    parser.add_argument('--mago-build-description', help='Record compiler/profile overrides used for this executable')
    parser.add_argument('--psalm-root', type=Path, required=True, help='Psalm fork checkout with installed dependencies')
    parser.add_argument('--runs', type=int, default=3, choices=range(1, 6))
    parser.add_argument('--output', type=Path, help='New directory for results')
    parser.add_argument('--yii2', action='store_true', help='Include isolated Yii fixture sources and enable both native Yii plugins')
    args = parser.parse_args()
    corpus = Path(__file__).resolve().parent
    fork = corpus.parents[1]
    psalm = args.psalm_root.resolve()
    if not (psalm / 'psalm').is_file() or not (psalm / 'vendor/autoload.php').is_file():
        parser.error('--psalm-root must contain psalm and vendor/autoload.php')
    mago = shutil.which(args.mago)
    if mago is None:
        parser.error('Mago executable was not found')
    mago = str(Path(mago).resolve())
    if any(shutil.which(tool) is None for tool in ['php', 'prlimit', 'timeout']) or not Path('/usr/bin/time').is_file():
        parser.error('PHP, prlimit, timeout, and GNU time are required')
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    output = args.output.resolve() if args.output else fork / '.mago/throws-benchmark' / stamp
    output.mkdir(parents=True, exist_ok=False)
    source_paths = [corpus / 'cases'] + ([corpus / 'yii2/cases'] if args.yii2 else [])
    dependency_paths = [corpus / 'dependencies'] + ([corpus / 'yii2/dependencies'] if args.yii2 else [])
    case_files = sorted(file for path in source_paths for file in path.glob('*.php'))
    dependency_files = sorted(file for path in dependency_paths for file in path.glob('*.php'))
    sources = case_files + dependency_files
    hashes = {str(file.relative_to(corpus)): hashlib.sha256(file.read_bytes()).hexdigest() for file in sources}
    versions = {
        'mago': subprocess.check_output([mago, '--version'], text=True).strip(),
        'psalm_commit': subprocess.check_output(['git', '-C', str(psalm), 'rev-parse', 'HEAD'], text=True).strip(),
        'php': subprocess.check_output(['php', '-r', 'echo PHP_VERSION;'], text=True).strip(),
        'mago_fork_commit': subprocess.check_output(['git', '-C', str(fork), 'rev-parse', 'HEAD'], text=True).strip(),
        'mago_executable_sha256': hashlib.sha256(Path(mago).read_bytes()).hexdigest(),
        'mago_build_description': args.mago_build_description,
    }
    samples = {'mago': [], 'psalm': []}
    for iteration in range(1, args.runs + 1):
        # An empty temporary project isolates Composer discovery and cache state.
        with tempfile.TemporaryDirectory(prefix='mago-throws-benchmark-') as temp:
            run_dir = Path(temp)
            config_mago = run_dir / 'mago.toml'
            config_mago.write_text(
                'php-version = "8.5.0"\nthreads = 1\n[source]\n'
                f'paths = {json.dumps([str(path) for path in source_paths])}\n'
                f'includes = {json.dumps([str(path) for path in dependency_paths])}\n'
                '[analyzer]\ncheck-throws = true\nfind-unused-definitions = false\nfind-unused-expressions = false\n'
                + ('plugins = ["yii2"]\n' if args.yii2 else '')
            )
            config_psalm = run_dir / 'psalm.xml'
            config_psalm.write_text(
                '<?xml version="1.0"?>\n'
                '<psalm xmlns="https://getpsalm.org/schema/config" errorLevel="8" phpVersion="8.5" '
                'resolveFromConfigFile="true" checkForThrowsDocblock="true" findUnusedCode="false" '
                'findUnusedVariablesAndParams="false" cacheDirectory="cache">\n'
                '<projectFiles>' + ''.join(f'<directory name="{html.escape(str(path), quote=True)}"/>' for path in source_paths) + '</projectFiles>\n'
                '<extraFiles>' + ''.join(f'<directory name="{html.escape(str(path), quote=True)}"/>' for path in dependency_paths) + '</extraFiles>\n'
                + ('<plugins><pluginClass class="Psalm\\Plugin\\Yii2\\Plugin"/></plugins>\n' if args.yii2 else '')
                +
                '<issueHandlers>\n'
                '<MissingThrowsDocblock errorLevel="error"/>\n'
                '<OverlyBroadThrowsDocblock errorLevel="error"/>\n'
                '<UnusedThrowsDocblock errorLevel="error"/>\n'
                '<MissingPureAnnotation errorLevel="suppress"/>\n'
                '<MissingImmutableAnnotation errorLevel="suppress"/>\n'
                '</issueHandlers>\n</psalm>\n'
            )
            commands = {
                'mago': [mago, '--workspace', str(run_dir), '--config', str(config_mago), '--threads', '1', '--colors', 'never', 'analyze', '--no-extensions', '--reporting-format', 'json'],
                'psalm': ['php', '-d', 'memory_limit=512M', '-d', 'opcache.enable_cli=0', str(psalm / 'psalm'), '-c', str(config_psalm), '--memory-limit=512M', '--threads=1', '--scan-threads=1', '--no-cache', '--no-diff', '--no-progress', '--output-format=json'],
            }
            # Alternate order, keep CPU and memory measurements sequential.
            order = ['mago', 'psalm'] if iteration % 2 else ['psalm', 'mago']
            for name in order:
                samples[name].append(run_analyzer(name, commands[name], run_dir, output, iteration))
    for file in sources:
        if hashlib.sha256(file.read_bytes()).hexdigest() != hashes[str(file.relative_to(corpus))]:
            raise RuntimeError(f'An analyzer changed a fixture: {file}')
    report = {'versions': versions, 'fixture_hashes': hashes, 'runs': args.runs, 'source_files': len(case_files), 'dependency_files': len(dependency_files), 'limits': {'threads': 1, 'address_space_mib': 1024, 'psalm_php_memory_mib': 512, 'wall_seconds': 40}, 'analyzers': {}}
    for name, runs in samples.items():
        signatures = [throws_by_file(name, sample['issues']) for sample in runs]
        if any(signature != signatures[0] for signature in signatures[1:]):
            raise RuntimeError(f'{name} returned inconsistent throws diagnostics across runs')
        report['analyzers'][name] = {
            'median_seconds': statistics.median(run['elapsed_seconds'] for run in runs),
            'median_max_rss_mib': statistics.median(run['max_rss_kib'] for run in runs) / 1024,
            'samples': [{key: value for key, value in run.items() if key != 'issues'} for run in runs],
            'throws_by_file': signatures[0],
        }
    (output / 'summary.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(f'Results: {output / "summary.json"}', flush=True)
    for name, metrics in report['analyzers'].items():
        print(f'{name} median: {metrics["median_seconds"]:.2f}s, {metrics["median_max_rss_mib"]:.1f} MiB', flush=True)
    print('Performance is a baseline comparison, not throws feature parity.', flush=True)


if __name__ == '__main__':
    main()
