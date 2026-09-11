"""Owned release identity/byte-boundary regression; never calls Git/network."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('release_metadata', Path(__file__).resolve().parents[1]/'scripts/release_metadata.py')
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='files-release-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root/'docs/releases').mkdir(parents=True)
        (self.root/'docs/licenses').mkdir()
        (self.root/'Cargo.toml').write_text('[package]\nversion="0.4.0-beta.1"\n')
        (self.root/'Cargo.lock').write_text('[[package]]\nname="ssh-files"\nversion="0.4.0-beta.1"\n')
        (self.root/'docs/releases/0.4.0-beta.1.md').write_text('Exact notes')
        for name in ('LICENSE', 'docs/licenses/THIRD_PARTY_NOTICES.txt', 'install-beta.ps1', 'install-beta.sh'):
            (self.root/name).write_bytes(b'Exact LF notice\n')
        self.dist = self.root/'dist'
        self.dist.mkdir()
        for name, source in metadata.NOTICES.items():
            (self.dist/name).write_bytes((self.root/source).read_bytes())
        for target in metadata.TARGETS:
            name = metadata.asset(target)
            (self.dist/name).write_bytes(target.encode())
            (self.dist/(name+'.sha256')).write_bytes((metadata.sha(self.dist/name)+'  '+name+'\n').encode())
            metadata.qualify(self.root, self.dist, 'a'*40, target, 'v0.4.0-beta.1')

    def test_complete_record_is_immutable_and_exact(self):
        metadata.record(self.root, self.dist, 'a'*40, 'v0.4.0-beta.1')
        value = json.loads((self.dist/'release-record.json').read_text())
        self.assertEqual(len(value['matrix']), 5)
        self.assertEqual(value['publication_policy'], 'prerelease; latest=false')
        self.assertEqual(value['stable_default'], '0.3.0')
        self.assertEqual(len(value['artifacts']), 17)
        with self.assertRaises((ValueError, FileExistsError)):
            metadata.record(self.root, self.dist, 'a'*40, 'v0.4.0-beta.1')

    def test_changed_binary_and_extra_payload_refused(self):
        path = self.dist/metadata.asset(metadata.TARGETS[0])
        path.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'Changed qualified'):
            metadata.record(self.root, self.dist, 'a'*40, 'v0.4.0-beta.1')
        path.write_bytes(metadata.TARGETS[0].encode())
        (self.dist/'private-config').write_text('not a release payload')
        with self.assertRaisesRegex(ValueError, 'unexpected'):
            metadata.record(self.root, self.dist, 'a'*40, 'v0.4.0-beta.1')

    def test_tag_lock_and_channel_validation(self):
        for version in ('latest', '0.04.0-beta.1', '0.4.0-beta.0', '0.4.0-beta.1\n'):
            with self.assertRaises(ValueError): metadata.channel(version)
        with self.assertRaises(ValueError): metadata.identity(self.root, 'v0.3.0')
        (self.root/'Cargo.lock').write_text('[[package]]\nname="ssh-files"\nversion="0.3.0"\n')
        with self.assertRaises(ValueError): metadata.identity(self.root)

    def test_wrong_source_or_missing_gate_refused(self):
        receipt = self.dist/f'qualification-{metadata.TARGETS[0]}.json'
        value = json.loads(receipt.read_text())
        original = dict(value, checks=list(value['checks']))
        value['checks'].pop()
        receipt.write_text(json.dumps(value))
        with self.assertRaises(ValueError):
            metadata.record(self.root, self.dist, 'a'*40, 'v0.4.0-beta.1')
        receipt.write_text(json.dumps(original))
        with self.assertRaises(ValueError):
            metadata.record(self.root, self.dist, 'b'*40, 'v0.4.0-beta.1')

    def test_notice_bytes_and_exact_sidecar(self):
        sidecar = self.dist/(metadata.asset(metadata.TARGETS[0])+'.sha256')
        original = sidecar.read_bytes()
        sidecar.write_bytes(original.replace(b'  ssh-files', b'  other-files'))
        with self.assertRaisesRegex(ValueError, 'sidecar'):
            metadata.qualify(self.root, self.dist, 'a'*40, metadata.TARGETS[0])
        sidecar.write_bytes(original)
        (self.dist/'LICENSE.txt').write_bytes(b'Exact LF notice\r\n')
        with self.assertRaisesRegex(ValueError, 'Notices'):
            metadata.qualify(self.root, self.dist, 'a'*40, metadata.TARGETS[0])


if __name__ == '__main__': unittest.main()
