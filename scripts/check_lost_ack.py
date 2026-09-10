"""Verify an owned real SFTP server whose successful hardlink ACK is withheld.

The caller owns the server and test-only transparent packet relay. This harness
only creates unique files inside the supplied same-machine fixture directory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import uuid


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def process_alive(pid):
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle.restype = wintypes.BOOL
        handle = kernel.OpenProcess(0x100000, False, pid)
        if not handle:
            assert ctypes.get_last_error() == 87, 'Cannot inspect owned fixture PID'
            return False
        try:
            status = kernel.WaitForSingleObject(handle, 0)
            assert status in (0, 258), status
            return status == 258
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--remote-root', type=Path, required=True)
    parser.add_argument('--ack-marker', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    options = parser.parse_args()
    binary = options.binary.resolve(strict=True)
    config = options.config.resolve(strict=True)
    remote = options.remote_root.resolve(strict=True)
    own = config.parent / ('ack-proof-' + uuid.uuid4().hex)
    own.mkdir()
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    source = own / 'source.bin'
    source.write_bytes(bytes(range(256)) * 1024)
    expected = digest(source)
    checks = []
    argv = [str(binary), '--host', 'fixture', '--config', str(config)]

    for mode in ('deadline', 'cancel'):
        final = remote / ('lost-ack-' + mode + '-' + uuid.uuid4().hex)
        old_marker = options.ack_marker.stat().st_mtime_ns if options.ack_marker.exists() else 0
        action = [] if mode == 'deadline' else ['--cancel-after-ms', '1500']
        start = time.monotonic()
        result = subprocess.run([*argv, *action, 'upload', str(source), final.as_posix()],
                                capture_output=True, text=True, encoding='utf-8',
                                timeout=22, creationflags=flags)
        elapsed = time.monotonic() - start
        value = json.loads(result.stdout)
        assert result.returncode != 0 and not value['ok'], (value, result.stderr)
        assert value['completion_unknown'] and value['outcome']['phase'] == 'committing', value
        assert value['cleanup_error'] is None, value
        assert options.ack_marker.stat().st_mtime_ns > old_marker, 'No new successful server response was suppressed'
        marker = json.loads(options.ack_marker.read_text(encoding='utf-8'))
        assert marker['event'] == 'actual-server-successful-hardlink-status-suppressed' and marker['status'] == 0, marker
        cleanup_deadline = time.monotonic() + 4
        while any(process_alive(marker[key]) for key in ('relay_pid', 'server_pid')):
            assert time.monotonic() < cleanup_deadline, 'Owned server-session relay or SFTP child survived disconnect'
            time.sleep(.02)
        assert digest(final) == expected, 'Actual committed destination was removed or changed'
        partial = Path(value['outcome']['partial'])
        assert partial.parent.resolve() == remote and digest(partial) == expected, value
        if mode == 'deadline':
            assert elapsed < 19, elapsed
        else:
            assert 'Cancelled' in value['error'] and elapsed < 5, (value, elapsed)
        # A different session can still read the actual committed destination.
        inspect = subprocess.run([*argv, 'stat', final.as_posix()], capture_output=True,
                                 text=True, encoding='utf-8', timeout=20, creationflags=flags)
        inspected = json.loads(inspect.stdout)
        assert inspect.returncode == 0 and inspected['ok'] and inspected['cleanup_error'] is None, inspected
        checks.append({'mode': mode, 'elapsed_s': elapsed, 'sha256': expected,
                       'result': value, 'server_marker': marker})
        print(f'PASS: lost successful hardlink ACK + {mode} reports completion unknown; '
              f'actual final and partial hashes preserved; session workers exit; separate SFTP session works ({elapsed:.2f}s)', flush=True)

    if options.report:
        options.report.parent.mkdir(parents=True, exist_ok=True)
        options.report.write_text(json.dumps({'checks': checks}, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
