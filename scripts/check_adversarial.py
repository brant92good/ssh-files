"""Owned-fixture trust, real ProxyCommand, stalled/cancelled transfer isolation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import struct
import subprocess
import sys
import threading
import time
import uuid
from transport_gate import TransportGate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--remote-root', type=Path, required=True, help='Owned same-machine SFTP root for independent assertions')
    parser.add_argument('--report', type=Path)
    options = parser.parse_args()
    root = options.config.resolve().parent
    remote = options.remote_root.resolve()
    binary = options.binary.resolve()
    config_text = options.config.read_text(encoding='utf-8')
    port = int(re.search(r'(?im)^\s*Port\s+(\d+)', config_text)[1])
    flags = subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0
    own = root/('adversarial-' + uuid.uuid4().hex)
    own.mkdir()
    report = []

    def passed(text):
        report.append(text)
        print('PASS: ' + text, flush=True)

    def call(config, *args, okay=True):
        result = subprocess.run([str(binary), '--host', 'fixture', '--config', str(config), *map(str, args)],
                                capture_output=True, text=True, encoding='utf-8', timeout=45, creationflags=flags)
        value = json.loads(result.stdout)
        assert value['ok'] == okay and (result.returncode == 0) == okay, (value, result.stderr)
        return value

    # Keep one entirely separate real SSH/SFTP session alive through the tests.
    control = subprocess.Popen(['ssh', '-T', '-s', '-oBatchMode=yes', '-oStrictHostKeyChecking=yes',
                                '-oControlMaster=no', '-oControlPath=none', '-F', str(options.config), 'fixture', 'sftp'],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=flags)

    def packet():
        results = queue.Queue()
        def read():
            try:
                header = control.stdout.read(4)
                assert len(header) == 4
                length = struct.unpack('>I', header)[0]
                assert 0 < length <= 262144
                body = control.stdout.read(length)
                assert len(body) == length
                results.put(body)
            except Exception as error:
                results.put(error)
        thread = threading.Thread(target=read)
        thread.start()
        try:
            result = results.get(timeout=5)
        except queue.Empty:
            control.kill()
            control.wait(timeout=2)
            thread.join(timeout=2)
            raise AssertionError('Independent control SSH session stopped responding')
        thread.join(timeout=2)
        assert not isinstance(result, Exception), result
        return result

    def control_stat():
        path = remote.as_posix().encode()
        payload = struct.pack('>BII', 17, 123, len(path)) + path
        control.stdin.write(struct.pack('>I', len(payload)) + payload)
        control.stdin.flush()
        assert packet()[0] == 105, 'Independent SFTP stat did not return attributes'

    gate = TransportGate(('127.0.0.1', port))
    child = None
    try:
        control.stdin.write(b'\0\0\0\5\1\0\0\0\3')
        control.stdin.flush()
        assert packet()[0] == 2
        # The alias still provides user, port, key and trust, while an explicit
        # selected address takes precedence over its stale HostName value.
        override_config = own/'hostname_override_config'
        override_config.write_text(re.sub(r'(?im)^\s*HostName.*$', ' HostName stale-route.invalid', config_text), encoding='utf-8')
        call(override_config, '--hostname', '127.0.0.1', 'stat', remote.as_posix())
        passed('explicit selected hostname overrides stale alias address while keeping its user, port, key and strict trust')

        empty_known = own/'empty_known_hosts'
        empty_known.write_text('')
        bad_config = own/'unknown_config'
        bad_config.write_text(re.sub(r'(?im)^\s*UserKnownHostsFile.*$', ' UserKnownHostsFile "' + empty_known.as_posix() + '"', config_text))
        before = set(remote.iterdir())
        denied = call(bad_config, 'stat', remote.as_posix(), okay=False)
        assert 'Host key verification failed' in denied['error'], denied
        assert set(remote.iterdir()) == before
        control_stat()
        passed('unknown host key is rejected without file writes; an already open independent SFTP session still works')

        changed_known = own/'changed_known_hosts'
        changed_known.write_text(f'[127.0.0.1]:{port} ' + ' '.join((root/'client.pub').read_text().split()[:2]) + '\n')
        changed_config = own/'changed_config'
        changed_config.write_text(re.sub(r'(?im)^\s*UserKnownHostsFile.*$', ' UserKnownHostsFile "' + changed_known.as_posix() + '"', config_text))
        denied = call(changed_config, 'stat', remote.as_posix(), okay=False)
        assert 'REMOTE HOST IDENTIFICATION HAS CHANGED' in denied['error'], denied
        assert set(remote.iterdir()) == before
        control_stat()
        passed('changed host key is rejected without weakening trust or writing files')

        relay = own/'proxy.py'
        relay.write_text('''import socket,sys,threading
s=socket.create_connection(('127.0.0.1',int(sys.argv[1])))
def upload():
 try:
  while True:
   data=sys.stdin.buffer.read1(65536)
   if not data: break
   s.sendall(data)
 finally:
  try: s.shutdown(socket.SHUT_WR)
  except OSError: pass
t=threading.Thread(target=upload,daemon=True);t.start()
try:
 while True:
  data=s.recv(65536)
  if not data: break
  sys.stdout.buffer.write(data);sys.stdout.buffer.flush()
finally: s.close()
''')
        proxy_config = own/'proxy_config'
        known = own/'proxy_known_hosts'
        known.write_text('proxy-fixture ' + ' '.join((root/'host.pub').read_text().split()[:2]) + '\n')
        text = re.sub(r'(?im)^\s*HostName.*$', ' HostName direct-connection-must-not-work.invalid', config_text)
        text = re.sub(r'(?im)^\s*UserKnownHostsFile.*$', ' UserKnownHostsFile "' + known.as_posix() + '"', text)
        # Only trusted fixture paths/port are formatted into its deliberate SSH
        # ProxyCommand. Production file paths never enter this command string.
        if os.name == 'nt':
            command = subprocess.list2cmdline([sys.executable, str(relay), str(gate.port)])
        else:
            import shlex
            command = shlex.join([sys.executable, str(relay), str(gate.port)])
        proxy_config.write_text(text + '\n HostKeyAlias proxy-fixture\n ProxyCommand ' + command + '\n')
        call(proxy_config, 'stat', remote.as_posix())
        assert gate.count() > 0
        passed('actual system OpenSSH executes an explicit ProxyCommand and strict HostKeyAlias trust with no direct-route fallback')

        source = own/'large-source'
        with source.open('wb') as output:
            for _ in range(128):
                output.write(bytes(range(256)) * 1024)
        for mode in ('cancel', 'idle-timeout'):
            target = remote/('paused-' + uuid.uuid4().hex)
            before = set(remote.glob('.ssh-files-*.partial'))
            argv = [str(binary), '--host', 'fixture', '--config', str(proxy_config)]
            if mode == 'cancel':
                argv += ['--cancel-after-ms', '1500']
            child = subprocess.Popen([*argv, 'upload', str(source), target.as_posix()], stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, encoding='utf-8', creationflags=flags)
            deadline = time.monotonic() + 10
            while set(remote.glob('.ssh-files-*.partial')) == before:
                assert child.poll() is None and time.monotonic() < deadline, 'Upload did not start'
                time.sleep(.001)
            gate.paused.set()
            start = time.monotonic()
            control_stat()
            stdout, stderr = child.communicate(timeout=36)
            elapsed = time.monotonic() - start
            assert child.returncode != 0, (stdout, stderr)
            value = json.loads(stdout)
            assert value['cleanup_error'] is None and not value['completion_unknown'], value
            assert not target.exists()
            assert value['outcome']['partial'], value
            if mode == 'cancel':
                assert 'Cancelled' in value['error'] and elapsed < 4, (value, elapsed)
            else:
                assert 'no progress' in value['error'] or 'timed out' in value['error'], value
                assert elapsed < 34, elapsed
            child = None
            gate.paused.clear()
            control_stat()
            passed(f'{mode} during a real stalled SSH upload stops owned transport; independent live SFTP session survives ({elapsed:.2f}s)')
    finally:
        if child and child.poll() is None:
            child.kill()
            child.wait(timeout=3)
        gate.close()
        control.kill()
        control.wait(timeout=3)
        for pipe in (control.stdin, control.stdout, control.stderr):
            pipe.close()
    if options.report:
        options.report.parent.mkdir(parents=True, exist_ok=True)
        options.report.write_text(json.dumps({'checks':report}, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
