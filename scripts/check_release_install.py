"""Exercise the actual tagged HTTPS installer with no live PATH/SSH changes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version',default='0.2.0')
    parser.add_argument('--ref')
    parser.add_argument('--report',type=Path)
    args=parser.parse_args()
    version,ref=args.version,args.ref or 'v'+args.version
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:[-.][A-Za-z0-9.-]+)?',version)
    assert re.fullmatch(r'[A-Za-z0-9._/-]+',ref)
    extension='ps1' if os.name == 'nt' else 'sh'
    url=f'https://raw.githubusercontent.com/brant92good/ssh-files/{ref}/install.{extension}'
    flags=getattr(subprocess,'CREATE_NO_WINDOW',0)
    with tempfile.TemporaryDirectory(prefix='files-https-') as name:
        root=Path(name)/"space \u6e2c\u8a66 ' release"; root.mkdir()
        installed=root/'app'
        env=dict(os.environ,SSH_FILES_INSTALL_DIR=str(installed),SSH_FILES_NO_PATH='1',SSH_FILES_VERSION=version,PYTHONHOME=str(root/'missing-python'),PYTHONPATH=str(root/'shadow'),VIRTUAL_ENV=str(root/'missing-venv'),CONDA_PREFIX=str(root/'missing-conda'))
        env.pop('SSH_FILES_BINARY',None); env.pop('SSH_FILES_SHA256',None)
        command=(['powershell.exe','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-Command',f'irm {url} | iex'] if os.name == 'nt' else ['sh','-c',f'curl -fsSL {url} | sh'])
        def install(success=True):
            result=subprocess.run(command,env=env,capture_output=True,encoding='utf-8',errors='replace',timeout=180,creationflags=flags)
            assert (result.returncode == 0) == success,(result.stdout,result.stderr)
        executable=installed/('bin/ssh-files.exe' if os.name == 'nt' else 'bin/ssh-files')
        install()
        assert subprocess.check_output([str(executable),'--version'],env=env,encoding='utf-8',timeout=10,creationflags=flags).strip() == 'ssh-files '+version
        digest=hashlib.sha256(executable.read_bytes()).hexdigest()
        sentinel=installed/'user-note.txt'; sentinel.write_text('Preserve on update')
        install(); assert sentinel.read_text() == 'Preserve on update'
        env['SSH_FILES_SHA256']='0'*64; install(success=False)
        assert hashlib.sha256(executable.read_bytes()).hexdigest() == digest
        assert not (installed/'python').exists() and not (installed/'uv').exists()
        result={'ok':True,'version':version,'installer':url,'sha256':digest,'checks':['actual HTTPS install','update','checksum rejection','user-file preservation','polluted environment'],'platform':os.name}
        print(json.dumps(result))
        if args.report:
            args.report.parent.mkdir(parents=True,exist_ok=True)
            args.report.write_text(json.dumps(result,indent=2),encoding='utf-8')


if __name__ == '__main__': main()
