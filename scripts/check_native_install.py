"""Fresh binary install/update in owned directories; no SSH or runtime Python."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--with-path', action='store_true', help='Disposable CI runner only')
    args = parser.parse_args()
    if args.with_path:
        assert os.environ.get('CI') == 'true', '--with-path is for disposable CI runners only'
    source = Path(__file__).resolve().parents[1]
    version = tomllib.loads((source/'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    binary = args.binary.resolve(strict=True)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    with tempfile.TemporaryDirectory(prefix='files-install-') as temp:
        root = Path(temp)/"space \u6e2c\u8a66 and ' quote"
        root.mkdir()
        install_dir, home = root/'app', root/'home'
        home.mkdir()
        (home/'.ssh').mkdir()
        ssh = home/'.ssh/config'
        ssh.write_text('Host fixture\n HostName fixture.invalid\n', encoding='utf-8')
        for name in ('.bashrc','.bash_login','.profile'):
            (home/name).write_text('# existing settings\n', encoding='utf-8')
        environment = dict(os.environ, SSH_FILES_INSTALL_DIR=str(install_dir), SSH_FILES_BINARY=str(binary), SSH_FILES_SHA256=digest, SSH_FILES_NO_PATH='0' if args.with_path else '1', SSH_FILES_VERSION=version,
                           PYTHONHOME=str(root/'missing-python'), PYTHONPATH=str(root/'shadow'), VIRTUAL_ENV=str(root/'missing-venv'), CONDA_PREFIX=str(root/'missing-conda'))
        if os.name != 'nt':
            environment.update(HOME=str(home), SHELL='/bin/bash')
        def install(expected=digest, destination=install_dir, success=True):
            env = dict(environment, SSH_FILES_SHA256=expected, SSH_FILES_INSTALL_DIR=str(destination))
            command = (['powershell.exe','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',str(source/'install.ps1')] if os.name == 'nt' else ['sh',str(source/'install.sh')])
            result = subprocess.run(command, env=env, capture_output=True, encoding='utf-8', errors='replace', timeout=45, creationflags=flags)
            assert (result.returncode == 0) == success, (result.stdout,result.stderr)
        executable = install_dir/('bin/ssh-files.exe' if os.name == 'nt' else 'bin/ssh-files')
        def run(*arguments):
            result = subprocess.run([str(executable),*arguments],env=environment,cwd=root,capture_output=True,encoding='utf-8',timeout=10,creationflags=flags)
            assert result.returncode == 0,(result.stdout,result.stderr)
            return result.stdout
        install()
        assert run('--version').strip() == 'ssh-files '+version
        assert '--host' in run('--help')
        assert not (install_dir/'python').exists() and not (install_dir/'uv').exists()
        sentinel = install_dir/'user-note.txt'
        sentinel.write_text('Preserve on update',encoding='utf-8')
        before = ssh.read_bytes()
        install()
        assert sentinel.read_text(encoding='utf-8') == 'Preserve on update' and ssh.read_bytes() == before
        install(expected='0'*64,success=False)
        assert hashlib.sha256(executable.read_bytes()).hexdigest() == digest
        assert run('--version').strip() == 'ssh-files '+version
        collision=root/'not-owned'; collision.mkdir(); (collision/'keep.txt').write_text('Other project')
        install(destination=collision,success=False)
        assert (collision/'keep.txt').read_text() == 'Other project'
        if args.with_path:
            if os.name == 'nt':
                import winreg
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER,'Environment') as key: value=winreg.QueryValueEx(key,'Path')[0]
                assert sum(Path(part).resolve() == (install_dir/'bin').resolve() for part in value.split(';') if part) == 1
            else:
                expected=str(install_dir/'bin/ssh-files')
                for mode in (['-lc'],['--noprofile','-ic']):
                    result=subprocess.run(['/bin/bash',*mode,'command -v ssh-files'],env=dict(os.environ,HOME=str(home),PATH='/usr/bin:/bin'),capture_output=True,encoding='utf-8',timeout=10)
                    assert result.returncode == 0 and result.stdout.strip() == expected,(result.stdout,result.stderr)
                assert not (home/'.bash_profile').exists()
                assert (home/'.profile').read_text() == '# existing settings\n'
                for name in ('.bashrc','.bash_login'): assert (home/name).read_text().count('# ssh-files') == 1
    print('PASS: fresh install/update, checksum rejection, Unicode/quoted paths, directory ownership, SSH/user-file preservation, polluted environment; no Python/Git/Cargo runtime')


if __name__ == '__main__':
    main()
