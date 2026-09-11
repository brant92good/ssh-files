"""Package only the exact tested UI executable, its checksum and notices."""
import argparse
import hashlib
from pathlib import Path
import subprocess
import tomllib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--target', required=True, choices=[
    'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-musl',
    'aarch64-unknown-linux-musl', 'aarch64-apple-darwin', 'x86_64-apple-darwin'])
parser.add_argument('--output', type=Path, default=Path('dist'))
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
version = tomllib.loads((root/'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
suffix = '.exe' if 'windows' in args.target else ''
source = root/'target'/args.target/'release'/('ssh-files'+suffix)
assert subprocess.check_output([str(source), '--version'], encoding='utf-8', timeout=10).strip() == 'ssh-files '+version
directory = args.output
directory.mkdir(parents=True, exist_ok=True)
binary = directory/('ssh-files-'+args.target+suffix)
with binary.open('xb') as stream:
    stream.write(source.read_bytes())
binary.chmod(0o755)
digest = hashlib.sha256(binary.read_bytes()).hexdigest()
with Path(str(binary)+'.sha256').open('x', encoding='ascii', newline='\n') as stream:
    stream.write(digest+'  '+binary.name+'\n')
for original, name in [('LICENSE', 'LICENSE.txt'), ('docs/licenses/THIRD_PARTY_NOTICES.txt', 'THIRD_PARTY_NOTICES.txt')]:
    with (directory/name).open('xb') as stream:
        stream.write((root/original).read_bytes())
print(binary.name+'  '+digest)
