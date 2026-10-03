#!/usr/bin/env python3
"""Package a compiled target using the layout expected by the Composer launcher."""

import argparse
import json
import tarfile
import tempfile
import shutil
import zipfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('target')
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
version = json.loads((root / 'composer.json').read_text())['extra']['mago-binary-version']
extension = '.exe' if args.target.endswith('windows-msvc') else ''
binary = root / 'target' / args.target / 'release' / f'mago{extension}'
directory = f'mago-{version}-{args.target}'
destination = root / 'dist'
destination.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='mago-package-') as temporary:
    package = Path(temporary) / directory
    package.mkdir()
    shutil.copy2(binary, package / binary.name)
    for license_file in ('LICENSE-MIT', 'LICENSE-APACHE'):
        shutil.copy2(root / license_file, package / license_file)
    if extension:
        archive = destination / f'{directory}.zip'
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for file in package.iterdir():
                output.write(file, f'{directory}/{file.name}')
    else:
        archive = destination / f'{directory}.tar.gz'
        with tarfile.open(archive, 'w:gz') as output:
            output.add(package, arcname=directory)
print(archive)
