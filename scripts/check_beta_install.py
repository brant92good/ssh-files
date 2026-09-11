"""Owned beta installer boundary checks; no network, UI, SSH or PATH writes."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(binary, shell):
    source = Path(__file__).resolve().parents[1]
    version = tomllib.loads((source/'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    cases = []
    with tempfile.TemporaryDirectory(prefix='files-beta-install-') as temporary:
        root = Path(temporary).resolve()/"long directory space 測試 ' quote"
        root.mkdir()
        home = root/'home'; home.mkdir()
        local = root/'local-app-data'; local.mkdir()
        xdg = home/'share'; xdg.mkdir()
        install_dir = root/'beta'
        ssh = home/'.ssh'; ssh.mkdir()
        (ssh/'config').write_bytes(b'Host fixture\n HostName example.invalid\n')
        for name in ('.bashrc', '.profile', '.zshrc'):
            (home/name).write_bytes(b'# owned unchanged profile\n')
        preserved = {path: path.read_bytes() for path in home.rglob('*') if path.is_file()}
        original_path = os.environ.get('PATH')
        registry_path = None
        if os.name == 'nt':
            import winreg
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, 'Environment') as key:
                try: registry_path = winreg.QueryValueEx(key, 'Path')
                except FileNotFoundError: pass
        environment = {key: value for key, value in os.environ.items() if not key.startswith('SSH_FILES_')}
        environment.update(SSH_FILES_BETA_VERSION=version, SSH_FILES_BETA_BINARY=str(binary),
                           SSH_FILES_BETA_SHA256=sha(binary), HOME=str(home), LOCALAPPDATA=str(local),
                           XDG_DATA_HOME=str(xdg), PYTHONHOME=str(root/'missing-python'),
                           PYTHONPATH=str(root/'shadow'), VIRTUAL_ENV=str(root/'missing-venv'))
        command = ([shell, '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', str(source/'install-beta.ps1')]
                   if os.name == 'nt' else ['sh', str(source/'install-beta.sh')])
        def install(name, destination=install_dir, success=True, **overrides):
            env = dict(environment, SSH_FILES_BETA_INSTALL_DIR=str(destination), **overrides)
            result = subprocess.run(command, env=env, capture_output=True, encoding='utf-8', errors='replace',
                                    timeout=45, creationflags=flags)
            assert (result.returncode == 0) == success, (name, result.stdout, result.stderr)
            cases.append({'case': name, 'result': 'pass', 'exit': result.returncode})
            return result
        exe = install_dir/('bin/ssh-files-beta.exe' if os.name == 'nt' else 'bin/ssh-files-beta')
        install('fresh explicit beta')
        assert sha(exe) == sha(binary)
        assert subprocess.check_output([str(exe), '--version'], env=environment, timeout=10,
                                       creationflags=flags).decode().strip() == 'ssh-files '+version
        note = install_dir/'user-note'; note.write_bytes(b'keep')
        install('owned update')
        assert note.read_bytes() == b'keep'
        for name, value in [('missing version', ''), ('implicit latest', 'latest'), ('stable version', '0.3.0'),
                            ('noncanonical beta', '0.04.0-beta.1'), ('wrong candidate version', '0.4.0-beta.999')]:
            install(name, success=False, SSH_FILES_BETA_VERSION=value)
            assert sha(exe) == sha(binary)
        install('tamper refusal', success=False, SSH_FILES_BETA_SHA256='0'*64)
        install('local input requires hash', success=False, SSH_FILES_BETA_SHA256='')
        assert sha(exe) == sha(binary)
        if os.name != 'nt':
            failed = root/'failed-version'
            failed.write_text(f"#!/bin/sh\nprintf '%s\\n' 'ssh-files {version}'\nexit 27\n")
            failed.chmod(0o755)
            install('correct version text but failed process', success=False,
                    SSH_FILES_BETA_BINARY=str(failed), SSH_FILES_BETA_SHA256=sha(failed))
            assert sha(exe) == sha(binary)
        unknown = root/'unknown'; unknown.mkdir(); (unknown/'sentinel').write_bytes(b'keep')
        install('unknown directory', unknown, False)
        assert (unknown/'sentinel').read_bytes() == b'keep'
        incomplete = root/'incomplete'; incomplete.mkdir()
        (incomplete/'.ssh-files-beta-installer').write_bytes(b'ssh-files-beta\n')
        install('partial ownership', incomplete, False)
        stable = local/'Programs/SSHFiles' if os.name == 'nt' else xdg/'ssh-files-install'
        stable.mkdir(parents=True)
        install('stable root exclusion', stable, False)
        if sys.platform == 'darwin':
            # The root is deliberately empty: no ownership marker can mask a
            # path-identity failure on the usual case-insensitive filesystem.
            alias = stable.with_name('SSH-FILES-INSTALL')
            install('Darwin stable case-alias exclusion', alias, False)
            assert list(stable.iterdir()) == []
        install('stable descendant exclusion', stable/'new-child', False)
        assert not (stable/'new-child').exists()
        workspace = root/'workspace long directory'; workspace.mkdir()
        install('explicit workspace exclusion', workspace/'new-child', False, SSH_FILES_WORKSPACE_ROOT=str(workspace))
        (workspace/'.workspace-installer').write_bytes(b'workspace')
        install('marked workspace exclusion', workspace/'new-child', False)
        if os.name == 'nt':
            kernel = ctypes.WinDLL('kernel32', use_last_error=True)
            kernel.GetShortPathNameW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32]
            buffer = ctypes.create_unicode_buffer(32768)
            size = kernel.GetShortPathNameW(str(workspace), buffer, len(buffer))
            assert size, ctypes.get_last_error()
            if buffer.value.casefold() != str(workspace).casefold():
                install('actual 8.3 workspace exclusion', Path(buffer.value)/'new-child', False,
                        SSH_FILES_WORKSPACE_ROOT=str(workspace))
                assert not (workspace/'new-child').exists()
            else: cases.append({'case': 'actual 8.3 workspace exclusion', 'result': 'skip: volume has no short alias'})
        link = root/'linked'
        try:
            link.symlink_to(workspace, target_is_directory=True)
        except OSError as error:
            if os.name != 'nt': raise
            cases.append({'case': 'symlink exclusion', 'result': 'skip: symlink privilege unavailable', 'error': str(error)})
        else:
            try: install('symlink exclusion', link/'new-child', False)
            finally: link.unlink()
        marker = install_dir/'.ssh-files-beta-installer'
        for path in (exe, marker, install_dir/'version'):
            outside = root/('hardlink-'+path.name)
            os.link(path, outside)
            before = outside.read_bytes()
            install('new inode replaces '+path.name)
            assert outside.read_bytes() == before and not os.path.samefile(path, outside)
        # Fail after binary replacement but before metadata replacement, and
        # require restoration of distinct previous bytes, not just a same-byte update.
        old = b'owned prior binary bytes; never executed'
        exe.write_bytes(old)
        if os.name == 'nt':
            kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p,
                                          ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
            kernel.CreateFileW.restype = ctypes.c_void_p
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            handle = kernel.CreateFileW(str(install_dir/'version'), 0x80000000, 1, None, 3, 0, None)
            assert handle != ctypes.c_void_p(-1).value, ctypes.get_last_error()
            try: install('publication failure rollback', success=False)
            finally: assert kernel.CloseHandle(handle)
        else:
            # Test-owned fault injection into one filesystem move only. All
            # other mv calls (including rollback) delegate to the actual tool.
            tools = root/'fault-tools'; tools.mkdir()
            real_mv = shutil.which('mv'); assert real_mv
            wrapper = tools/'mv'
            wrapper.write_text('#!/bin/sh\nfor arg do last=$arg; done\ncase "$1" in */.install-beta.*/version) if [ "$last" = "$FAIL_DEST" ]; then exit 73; fi;; esac\nexec "$REAL_MV" "$@"\n')
            wrapper.chmod(0o755)
            install('publication failure rollback', success=False, PATH=str(tools)+os.pathsep+environment['PATH'],
                    FAIL_DEST=str(install_dir/'version'), REAL_MV=real_mv)
        assert exe.read_bytes() == old
        assert marker.read_bytes() == b'ssh-files-beta\n'
        assert (install_dir/'version').read_text().strip() == version
        assert not (install_dir/'.install-beta.lock').exists()
        install('recovery after rolled-back failure')
        assert sha(exe) == sha(binary)
        for path, contents in preserved.items(): assert path.read_bytes() == contents
        assert os.environ.get('PATH') == original_path
        if os.name == 'nt':
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, 'Environment') as key:
                try: actual = winreg.QueryValueEx(key, 'Path')
                except FileNotFoundError: actual = None
            assert actual == registry_path
        cases.append({'case': 'SSH profiles user files and PATH unchanged', 'result': 'pass'})
    return {'shell': shell, 'binary_sha256': sha(binary), 'version': version, 'checks': cases, 'ok': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--powershell', default='powershell.exe')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    value = check(args.binary.resolve(strict=True), args.powershell if os.name == 'nt' else 'sh')
    print(json.dumps(value, indent=2))
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        with args.report.open('x', encoding='utf-8') as stream: json.dump(value, stream, indent=2)


if __name__ == '__main__': main()
