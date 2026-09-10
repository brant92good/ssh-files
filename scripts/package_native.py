"""Package only the exact tested UI executable, its checksum and notices."""
import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--target', required=True, choices=[
    'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
    'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin'])
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root/'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
suffix = '.exe' if 'windows' in args.target else ''
source = root/'target'/args.target/'release'/('ssh-files'+suffix)
assert subprocess.check_output([str(source), '--version'], encoding='utf-8').strip() == 'ssh-files '+version
directory = root/'dist'
directory.mkdir(exist_ok=True)
binary = directory/('ssh-files-'+args.target+suffix)
shutil.copy2(source, binary)
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
Path(str(binary)+'.sha256').write_text(digest+'  '+binary.name+'\n', encoding='ascii')
shutil.copy2(root/'LICENSE', directory/'LICENSE.txt')
shutil.copy2(root/'docs/licenses/THIRD_PARTY_NOTICES.txt', directory/'THIRD_PARTY_NOTICES.txt')
print(binary.name+'  '+digest)
