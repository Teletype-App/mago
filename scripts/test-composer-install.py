#!/usr/bin/env python3
"""Verify a fresh Composer installation downloads and runs the Teletype binary."""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--version', required=True)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='mago-composer-install-') as temporary:
    workspace = Path(temporary)
    (workspace / 'composer.json').write_text(json.dumps({
        'name': 'teletype/installation-probe',
        'repositories': [{'type': 'vcs', 'url': 'https://github.com/Teletype-App/mago'}],
        'require-dev': {'teletype/mago': args.version},
    }))
    environment = dict(os.environ)
    token = environment.get('GITHUB_TOKEN') or environment.get('GH_TOKEN')
    if token:
        environment['COMPOSER_AUTH'] = json.dumps({'github-oauth': {'github.com': token}})
    subprocess.run(['composer', 'install', '--no-interaction', '--no-progress'], cwd=workspace, env=environment, check=True)
    binary = workspace / 'vendor/bin/mago'
    version = subprocess.check_output(['php', str(binary), '--version'], cwd=workspace, env=environment, text=True).strip()
    assert version == f'mago {args.version}', version
    (workspace / 'probe.php').write_text('<?php function leaf(): void { throw new \\RuntimeException(); } function probe(): void { leaf(); }\n')
    (workspace / 'mago.toml').write_text(f'version = "{args.version.split("-", 1)[0]}"\nphp-version = "8.3.0"\n[source]\npaths = ["probe.php"]\n[analyzer]\ncheck-throws = true\nfind-unused-definitions = false\n')
    result = subprocess.run([
        'php', str(binary), '--threads', '1', 'analyze', '--no-extensions',
        '--throws-only', '--reporting-format', 'json',
    ], cwd=workspace, env=environment, capture_output=True, text=True, timeout=40)
    assert result.returncode == 1, result.stderr
    issues = json.loads(result.stdout)['issues']
    assert any(issue['code'] == 'unhandled-thrown-type' and '`probe`' in issue['message'] for issue in issues), issues
    assert (workspace / 'vendor/teletype/mago/schema.json').is_file()
    subprocess.run([
        'php', str(binary), 'self-update', '--to-project-version', '--check',
    ], cwd=workspace, env=environment, check=True)
    print(f'Fresh Composer installation and native throws analysis: passed ({version})')
