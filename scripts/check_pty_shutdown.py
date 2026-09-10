"""Linux-only, owned PTY EOF/TERM while actual SSH SFTP transfer is paused."""
import argparse
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import pyte
import re
import select
import signal
import shutil
import struct
import sys
import tempfile
import termios
import time
from transport_gate import TransportGate


def identity(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return fields[19], fields[0]
    except (FileNotFoundError, ProcessLookupError):
        return None


def running(pid, start):
    value = identity(pid)
    return value is not None and value[0] == start and value[1] != 'Z'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--remote-root', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    assert sys.platform.startswith('linux')
    binary = args.binary.resolve(strict=True)
    config = args.config.resolve(strict=True)
    original = config.read_text(encoding='utf-8')
    port = int(re.search(r'(?im)^\s*Port\s+(\d+)', original)[1])
    public = (config.parent/'host.pub').read_text(encoding='utf-8')
    ssh_executable = Path(shutil.which('ssh')).resolve(strict=True)
    report = []
    for mode in ('pty-eof', 'sigterm'):
        with tempfile.TemporaryDirectory(prefix='files-pty-') as name, tempfile.TemporaryDirectory(prefix='files-pty-', dir=args.remote_root) as remote_name:
            root, remote = Path(name), Path(remote_name)
            (remote/'browser-ready').write_bytes(b'ready')
            source = root/'large.bin'
            with source.open('wb') as stream:
                stream.truncate(32 * 1024 * 1024)
            gate = TransportGate(('127.0.0.1', port))
            known = root/'known_hosts'
            known.write_text('fixture-key ' + public, encoding='utf-8')
            selected = root/'client_config'
            text = re.sub(r'(?im)^\s*Port\s+\d+', f' Port {gate.port}', original)
            text = re.sub(r'(?im)^\s*UserKnownHostsFile.*$', f' UserKnownHostsFile {known}', text)
            selected.write_text(text + '\n HostKeyAlias fixture-key\n', encoding='utf-8')
            pid, master = pty.fork()
            if pid == 0:
                os.execve(binary, [str(binary), '--host', 'fixture', '--config', str(selected), '--local', str(root), '--remote', str(remote)], dict(os.environ, TERM='xterm-256color'))
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 130, 0, 0))
            ssh = {}
            reaped = False
            seen = bytearray()
            screen = pyte.Screen(130, 32)
            terminal_stream = pyte.Stream(screen)
            decoder = codecs.getincrementaldecoder("utf-8")("replace")
            def record_children():
                for children in Path(f'/proc/{pid}/task').glob('*/children'):
                    try:
                        candidates = children.read_text().split()
                    except FileNotFoundError:
                        continue
                    for child in candidates:
                        child = int(child)
                        try:
                            argv = Path(f'/proc/{child}/cmdline').read_bytes().split(b'\0')
                            executable = Path(f'/proc/{child}/exe').resolve(strict=True)
                            start = identity(child)
                            if executable == ssh_executable and os.fsencode(selected) in argv and start:
                                ssh[child] = start[0]
                        except FileNotFoundError:
                            pass
            def pump():
                if master is not None and select.select([master], [], [], .02)[0]:
                    try:
                        data = os.read(master, 65536)
                        seen.extend(data)
                        del seen[:-262144]
                        terminal_stream.feed(decoder.decode(data))
                    except OSError:
                        pass
            def expect(text):
                deadline = time.monotonic() + 15
                while text not in "\n".join(screen.display):
                    assert time.monotonic() < deadline, (text, "\n".join(screen.display))
                    pump()
            try:
                expect('browser-ready')
                os.write(master, b'\x1b[17~')
                expect('Upload local paths')
                os.write(master, f'\x1b[200~"{source}"\n\x1b[201~'.encode())
                expect(str(source))
                os.write(master, b'\x1b[15~')
                expect('Review transfers')
                os.write(master, b'\x1b[20~')
                deadline = time.monotonic() + 10
                while not list(remote.glob('.ssh-files-*.partial')):
                    assert time.monotonic() < deadline, seen[-8000:].decode(errors='replace')
                    pump()
                gate.paused.set()
                assert not (remote/'large.bin').exists()
                # Enumerate only direct children of this fixture's process
                # threads, then verify exact executable/config and start identity.
                record_children()
                assert len(ssh) == 2, ('Expected browser and transfer SSH children', ssh)
                assert all(running(child, start) for child, start in ssh.items()), 'Both owned SSH identities must be alive before the shutdown trigger'
                started = time.monotonic()
                if mode == 'pty-eof':
                    os.close(master)
                    master = None
                else:
                    os.kill(pid, signal.SIGTERM)
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline:
                    waited, status = os.waitpid(pid, os.WNOHANG)
                    if waited:
                        reaped = True
                        break
                    pump()
                    time.sleep(.01)
                assert reaped, f'{mode}: app failed to exit'
                assert os.waitstatus_to_exitcode(status) == 0, (mode, "App did not exit cleanly", status)
                for child, start in ssh.items():
                    assert not running(child, start), f'{mode}: owned SSH {child} survived'
                assert not (remote/'large.bin').exists()
                entry = {'case': mode, 'elapsed_s': time.monotonic()-started, 'ssh_children_stopped': len(ssh), 'status': os.waitstatus_to_exitcode(status), 'partial_reported_by_fixture': bool(list(remote.glob('.ssh-files-*.partial')))}
                report.append(entry)
                print('PASS:', json.dumps(entry), flush=True)
            finally:
                if not reaped:
                    # Include children even if an earlier setup/assertion failed,
                    # while the parent identity still owns its task/children tree.
                    record_children()
                    try:
                        if os.getpgid(pid) == pid and pid != os.getpgrp():
                            os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    os.waitpid(pid, 0)
                for child, start in ssh.items():
                    if running(child, start) and os.getpgid(child) == child and child != os.getpgrp():
                        os.killpg(child, signal.SIGKILL)
                if master is not None:
                    os.close(master)
                gate.close()
                if args.report:
                    args.report.parent.mkdir(parents=True, exist_ok=True)
                    args.report.with_name(f"{args.report.stem}-{mode}.raw").write_bytes(seen)
                    args.report.with_name(f"{args.report.stem}-{mode}.txt").write_text("\n".join(screen.display), encoding="utf-8")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
