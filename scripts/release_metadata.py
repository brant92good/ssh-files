"""Validate immutable Files release identity and exact pre-publication payloads.

This tool does not run gates itself. CI invokes qualification only after its
listed checks have passed. Published HTTPS results are separate later receipts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tomllib

TARGETS = ('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
           'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin')
BASE = r'(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)'
NOTICES = {'LICENSE.txt': 'LICENSE',
           'THIRD_PARTY_NOTICES.txt': 'docs/licenses/THIRD_PARTY_NOTICES.txt'}


def channel(version):
    if re.fullmatch(BASE, version):
        return 'stable'
    if re.fullmatch(BASE + r'-beta\.[1-9][0-9]*', version):
        return 'beta'
    raise ValueError('Use an exact x.y.z or x.y.z-beta.N version')


def identity(root, tag=None):
    version = tomllib.loads((root/'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    entries = tomllib.loads((root/'Cargo.lock').read_text(encoding='utf-8'))['package']
    if [item['version'] for item in entries if item['name'] == 'ssh-files'] != [version]:
        raise ValueError('Cargo package/lock versions disagree')
    value = {'version': version, 'tag': 'v'+version, 'channel': channel(version)}
    if tag is not None and tag != value['tag']:
        raise ValueError('Tag must exactly match the package version')
    if not (root/'docs/releases'/f'{version}.md').is_file():
        raise ValueError('Exact versioned release notes required')
    return value


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def asset(target):
    return 'ssh-files-'+target+('.exe' if target.endswith('windows-msvc') else '')


def checks(target, release_channel):
    result = ['fmt', 'all-target-tests', 'clippy', 'release-build',
              'release-metadata-tests', 'exact-notices', 'local-install-update']
    if release_channel == 'beta':
        result += ['beta-isolation-ownership-tamper-hardlink-rollback-fixtures']
    if target.endswith('windows-msvc'):
        result += ['ps51-and-ps7-installers'] if release_channel == 'beta' else []
    if 'linux' in target:
        result += ['actual-openssh-adversarial-ui-paste-shutdown', 'actual-lost-ack']
    return result


def write_new(path, value):
    with path.open('x', encoding='utf-8', newline='\n') as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write('\n')


def qualify(root, directory, source, target, tag=None):
    value = identity(root, tag)
    if target not in TARGETS or not re.fullmatch(r'[a-f0-9]{40}', source):
        raise ValueError('Exact source commit and supported target required')
    name = asset(target)
    if (directory/(name+'.sha256')).read_text(encoding='ascii') != f'{sha(directory/name)}  {name}\n':
        raise ValueError('Incorrect binary sidecar')
    for notice, original in NOTICES.items():
        if (directory/notice).read_bytes() != (root/original).read_bytes():
            raise ValueError('Notices must retain exact source bytes')
    names = [name, name+'.sha256', *NOTICES]
    value.update(schema_version=1, source_commit=source, target=target, result='pass',
                 checks=checks(target, value['channel']),
                 assets={item: sha(directory/item) for item in names})
    write_new(directory/f'qualification-{target}.json', value)


def record(root, directory, source, tag):
    value = identity(root, tag)
    if not re.fullmatch(r'[a-f0-9]{40}', source):
        raise ValueError('Exact source commit required')
    expected, matrix = set(NOTICES), []
    for target in TARGETS:
        receipt = f'qualification-{target}.json'
        proof = json.loads((directory/receipt).read_text(encoding='utf-8'))
        name = asset(target)
        if (any(proof.get(key) != item for key, item in value.items()) or
                proof.get('source_commit') != source or proof.get('target') != target or
                proof.get('schema_version') != 1 or proof.get('result') != 'pass' or
                proof.get('checks') != checks(target, value['channel']) or
                set(proof.get('assets', {})) != {name, name+'.sha256', *NOTICES}):
            raise ValueError('Qualification identity/gate matrix mismatch: '+target)
        for filename, digest in proof['assets'].items():
            if sha(directory/filename) != digest:
                raise ValueError('Changed qualified asset: '+filename)
        expected.update((receipt, name, name+'.sha256'))
        matrix.append(proof)
    if {path.name for path in directory.iterdir()} != expected:
        raise ValueError('Missing or unexpected release payload')
    for notice, original in NOTICES.items():
        if (directory/notice).read_bytes() != (root/original).read_bytes():
            raise ValueError('Published notices differ from tagged source')
    value.update(schema_version=1, source_commit=source, matrix=matrix,
                 artifacts={name: sha(directory/name) for name in sorted(expected)},
                 installers={name: sha(root/name) for name in ('install-beta.ps1', 'install-beta.sh')},
                 publication_policy='prerelease; latest=false', stable_default='0.3.0',
                 post_publication={'status_at_creation': 'required; not yet observed',
                                   'gate': 'actual tagged HTTPS fresh/update on five targets, both Windows shells',
                                   'evidence': 'separate released-install workflow receipts'},
                 scope={'persistent_controller': False, 'native_explorer_drop_qualified': False,
                        'macos': 'beta'})
    write_new(directory/'release-record.json', value)
    with (directory/'SHA256SUMS').open('x', encoding='ascii', newline='\n') as stream:
        stream.write(''.join(sha(path)+'  '+path.name+'\n' for path in sorted(directory.iterdir()) if path.name != 'SHA256SUMS'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--tag')
    parser.add_argument('--github-output', type=Path)
    parser.add_argument('--artifacts', type=Path, default=Path('dist'))
    parser.add_argument('--source', default=os.environ.get('GITHUB_SHA', ''))
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--qualify', choices=TARGETS)
    mode.add_argument('--record', action='store_true')
    args = parser.parse_args()
    value = identity(args.root, args.tag)
    if args.github_output:
        with args.github_output.open('a', encoding='utf-8') as stream:
            stream.write(f'version={value["version"]}\nchannel={value["channel"]}\n')
    if args.qualify:
        qualify(args.root, args.artifacts, args.source, args.qualify, args.tag)
    elif args.record:
        if args.tag is None:
            parser.error('--record requires --tag')
        record(args.root, args.artifacts, args.source, args.tag)
    print(json.dumps(value))


if __name__ == '__main__':
    main()
