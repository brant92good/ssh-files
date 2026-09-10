"""Qualify an explicitly supplied disposable SFTP server using the native proof.

The server/key/config lifetime is owned by the caller's fixture. This script
creates uniquely named test files only under its supplied owned remote root.
It does not inspect or use the user's SSH configuration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import uuid


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--host', required=True)
    parser.add_argument('--remote-root', required=True)
    parser.add_argument('--local-server-root', type=Path, help='Same owned remote root visible locally, for independent file/race assertions')
    parser.add_argument('--report', type=Path)
    options = parser.parse_args()
    binary = options.binary.resolve(strict=True)
    config = options.config.resolve(strict=True)
    nonce = uuid.uuid4().hex
    report = {'binary': str(binary), 'checks': [], 'commands': []}
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0

    def command(*args, okay=True, cancel=None):
        argv = [str(binary), '--host', options.host, '--config', str(config)]
        if cancel is not None:
            argv += ['--cancel-after-ms', str(cancel)]
        start = time.monotonic()
        result = subprocess.run([*argv, *map(str, args)], capture_output=True, text=True,
                                encoding='utf-8', timeout=45, creationflags=flags)
        elapsed = time.monotonic() - start
        assert result.stdout.strip(), (result.returncode, result.stderr)
        value = json.loads(result.stdout)
        assert value['ok'] == okay and (result.returncode == 0) == okay, (args, value, result.stderr)
        assert not value.get('cleanup_error'), value
        report['commands'].append({'action': args[0], 'elapsed_s': elapsed, 'result': value})
        return value

    def remote(name):
        return options.remote_root.rstrip('/') + '/' + nonce + '-' + name

    def passed(text):
        report['checks'].append(text)
        print('PASS: ' + text, flush=True)

    with tempfile.TemporaryDirectory(prefix='ssh-files-native-') as temp:
        root = Path(temp)
        command('stat', options.remote_root)
        for name, size in [('zero.txt', 0), ('space 開發.txt', 2731), ('literal$;&[].bin', 4 * 1024 * 1024)]:
            source = root/name
            source.write_bytes((bytes(range(256)) * ((size + 255) // 256))[:size])
            destination = remote(name)
            uploaded = command('upload', source, destination)
            assert uploaded['outcome']['phase'] == 'completed' and uploaded['outcome']['partial'] is None
            downloaded = root/('download-' + name)
            command('download', destination, downloaded)
            assert digest(downloaded) == digest(source)
            if options.local_server_root:
                assert digest(options.local_server_root/(nonce + '-' + name)) == digest(source)
        passed('zero-byte, Unicode/spaces and metacharacter multi-chunk files round-trip with identical SHA-256 hashes')

        source = root/'replacement'
        source.write_bytes(b'replacement-must-not-win')
        existing = remote('space 開發.txt')
        original = root/'space 開發.txt'
        collision = command('upload', source, existing, okay=False)
        assert collision['outcome']['phase'] == 'finalization-rejected' and collision['outcome']['partial']
        downloaded = root/'verify-original'
        command('download', existing, downloaded)
        assert digest(downloaded) == digest(original)
        downloaded.write_bytes(b'keep-local-original')
        rejected = command('download', existing, downloaded, okay=False)
        assert rejected['outcome']['phase'] == 'finalization-rejected'
        assert downloaded.read_bytes() == b'keep-local-original'
        passed('existing remote and local destinations survive collision unchanged; owned partial paths are reported')

        large = root/'large-source'
        # Enough data to cancel during work without allocating the whole file.
        with large.open('wb') as output:
            for _ in range(128):
                output.write(bytes(range(256)) * 1024)
        cancelled = command('upload', large, remote('cancelled'), okay=False, cancel=1)
        assert 'Cancelled' in cancelled['error'] and not cancelled['completion_unknown'], cancelled
        # A fast cancellation can win while the initial local source metadata
        # is pending, before a partial is requested. The stalled-transfer test
        # separately waits for a real partial before exercising cancellation.
        assert cancelled['outcome']['phase'] in ('connected', 'creating-partial', 'transferring', 'closing-partial'), cancelled
        if cancelled['outcome']['phase'] != 'connected':
            assert cancelled['outcome']['partial'], cancelled
        if options.local_server_root:
            assert not (options.local_server_root/(nonce + '-cancelled')).exists()
        passed('early upload cancellation closes its owned connection, reports any started partial, and publishes no final file')

        if options.local_server_root:
            marker = b'competing-writer-wins'
            racing_destination = options.local_server_root/(nonce + '-race.bin')
            ready = threading.Event()
            winner = []
            before = set(options.local_server_root.glob('.ssh-files-*.partial'))

            def competitor():
                ready.set()
                # Wait for this upload to create its partial. Creating the final
                # before starting upload would only repeat the collision test.
                deadline = time.monotonic() + 10
                while set(options.local_server_root.glob('.ssh-files-*.partial')) == before:
                    if time.monotonic() >= deadline:
                        winner.append('no-upload-observed')
                        return
                    time.sleep(.001)
                try:
                    with racing_destination.open('xb') as output:
                        output.write(marker)
                    winner.append('competitor')
                except FileExistsError:
                    winner.append('transfer')

            task = threading.Thread(target=competitor)
            task.start()
            ready.wait(2)
            result = command('upload', large, remote('race.bin'), okay=False)
            task.join(2)
            assert winner == ['competitor'] and result['outcome']['phase'] == 'finalization-rejected'
            assert racing_destination.read_bytes() == marker
            passed('independent exclusive destination creator is never replaced by finalization')

        # Independent connection after cancellation qualifies unrelated server
        # survival. A simultaneous surviving session is a separate fixture gate.
        command('stat', options.remote_root)
        passed('server remains available for a separate SSH/SFTP connection after cancellation')

    if options.report:
        options.report.parent.mkdir(parents=True, exist_ok=True)
        options.report.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding='utf-8')


if __name__ == '__main__':
    main()
