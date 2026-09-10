"""Collect the exact five-target Cargo.lock dependency notices for binary release."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
           'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin']


def main():
    used = {}
    for target in TARGETS:
        metadata = json.loads(subprocess.check_output(['cargo', '+1.94.0', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target], cwd=ROOT, encoding='utf-8'))
        nodes = {node['id']: node for node in metadata['resolve']['nodes']}
        packages = {package['id']: package for package in metadata['packages']}
        queue, seen = [metadata['resolve']['root']], set()
        while queue:
            ident = queue.pop()
            if ident in seen:
                continue
            seen.add(ident)
            used[ident] = packages[ident]
            queue.extend(dep['pkg'] for dep in nodes[ident]['deps'] if any(kind['kind'] != 'dev' for kind in dep['dep_kinds']))
    parts = ['SSH Files third-party notices\n\nGenerated from Cargo.lock for the five release targets, including build dependencies.\n']
    for package in sorted(used.values(), key=lambda package: (package['name'], package['version'])):
        if package['source'] is None:
            continue
        root = Path(package['manifest_path']).parent
        files = sorted(path for path in root.iterdir() if path.is_file() and path.name.lower().startswith(('license', 'copying', 'notice', 'copyright')))
        assert files, f"Review missing notices for {package['name']}"
        parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nDeclared license: {package['license']}\nSource: {package.get('repository') or 'crates.io/' + package['name']}\n")
        for path in files:
            parts.append(f'\n--- {path.name} ---\n' + path.read_text(encoding='utf-8'))
    output = ROOT/'docs'/'licenses'/'THIRD_PARTY_NOTICES.txt'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text('\n'.join(parts), encoding='utf-8')
    print(f'{output.name}: {len(used)-1} dependency distributions')


if __name__ == '__main__':
    main()
