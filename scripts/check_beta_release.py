"""Actual exact-tag HTTPS beta installation; no local binary/hash substitution."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import tempfile
import urllib.request


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--powershell', default='powershell.exe')
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    assert re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-beta\.[1-9][0-9]*', args.version)
    extension = 'ps1' if os.name == 'nt' else 'sh'
    installer = 'install-beta.'+extension
    tag = 'v'+args.version
    base = 'https://github.com/brant92good/ssh-files/releases/download/'+tag+'/'
    url = f'https://raw.githubusercontent.com/brant92good/ssh-files/{tag}/{installer}'
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    with tempfile.TemporaryDirectory(prefix='files-beta-https-') as temporary:
        root = Path(temporary).resolve()/"space 測試 ' release"; root.mkdir()
        script = root/installer
        script.write_bytes(urllib.request.urlopen(url, timeout=60).read())
        record = json.load(urllib.request.urlopen(base+'release-record.json', timeout=60))
        assert record['tag'] == tag and record['channel'] == 'beta' and record['version'] == args.version
        assert re.fullmatch('[a-f0-9]{40}', record['source_commit'])
        assert record['installers'][installer] == digest(script)
        home = root/'home'; home.mkdir()
        (home/'.profile').write_bytes(b'# unchanged shell profile\n')
        (home/'.ssh').mkdir(); (home/'.ssh/config').write_bytes(b'Host fixture\n HostName example.invalid\n')
        installed = root/'beta'
        env = {key: value for key, value in os.environ.items() if not key.startswith('SSH_FILES_')}
        env.update(SSH_FILES_BETA_VERSION=args.version, SSH_FILES_BETA_INSTALL_DIR=str(installed),
                   HOME=str(home), XDG_DATA_HOME=str(home/'share'), LOCALAPPDATA=str(root/'local'),
                   PYTHONHOME=str(root/'missing'), PYTHONPATH=str(root/'shadow'), VIRTUAL_ENV=str(root/'absent'))
        command = ([args.powershell, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', str(script)]
                   if os.name == 'nt' else ['sh', str(script)])
        def install(success=True, **override):
            result = subprocess.run(command, env=dict(env, **override), capture_output=True, encoding='utf-8',
                                    errors='replace', timeout=180, creationflags=flags)
            assert (result.returncode == 0) == success, (result.stdout, result.stderr)
        install()
        exe = installed/('bin/ssh-files-beta.exe' if os.name == 'nt' else 'bin/ssh-files-beta')
        actual_version = subprocess.check_output([str(exe), '--version'], env=env, timeout=10, creationflags=flags).decode().strip()
        assert actual_version == 'ssh-files '+args.version
        binary_hash = digest(exe)
        # Exactly one platform asset must account for these installed bytes.
        assets = [name for name, value in record['artifacts'].items()
                  if name.startswith('ssh-files-') and not name.endswith('.sha256') and value == binary_hash]
        assert len(assets) == 1, assets
        note = installed/'user-note'; note.write_bytes(b'keep on update')
        install(); assert note.read_bytes() == b'keep on update' and digest(exe) == binary_hash
        install(False, SSH_FILES_BETA_SHA256='0'*64)
        assert digest(exe) == binary_hash
        assert (home/'.profile').read_bytes() == b'# unchanged shell profile\n'
        assert (home/'.ssh/config').read_bytes() == b'Host fixture\n HostName example.invalid\n'
        result = {'ok': True, 'version': args.version, 'source_commit': record['source_commit'],
                  'installer': url, 'installer_sha256': digest(script), 'binary_sha256': binary_hash,
                  'asset': assets[0], 'platform': platform.platform(), 'shell': args.powershell if os.name == 'nt' else 'sh',
                  'checks': ['actual tagged HTTPS fresh install', 'same-tag update', 'tamper refusal',
                             'source installer and published binary hashes', 'user files and SSH config unchanged',
                             'polluted environment'], 'ui_or_ssh_launched': False}
        args.report.parent.mkdir(parents=True, exist_ok=True)
        with args.report.open('x', encoding='utf-8') as stream: json.dump(result, stream, indent=2)
        print(json.dumps(result, indent=2))


if __name__ == '__main__': main()
