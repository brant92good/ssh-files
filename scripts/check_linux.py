"""Own a disposable loopback OpenSSH SFTP server for the compiled proof."""
import argparse
import getpass
import os
from pathlib import Path
import shlex
import socket
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--sudo-sshd', action='store_true')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--adversarial', action='store_true')
    parser.add_argument('--lost-ack', action='store_true')
    parser.add_argument('--ui-binary', type=Path, help='Also qualify real OS PTY UI and EOF/TERM cleanup')
    options = parser.parse_args()
    if not sys.platform.startswith('linux'):
        parser.error('This owned server fixture is for Linux')
    binary = options.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='ssh-files-openssh-') as temporary:
        root = Path(temporary)
        remote = root/'remote space 開發'
        remote.mkdir()
        for name in ('host', 'client'):
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(root/name)], check=True, timeout=10)
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            port = probe.getsockname()[1]
        known = root/'known_hosts'
        known.write_text(f'[127.0.0.1]:{port} ' + (root/'host.pub').read_text())
        server_config = root/'sshd_config'
        forced_command = 'internal-sftp'
        if options.lost_ack:
            subsystem = Path('/usr/lib/openssh/sftp-server')
            assert subsystem.is_file(), 'Official OpenSSH SFTP server executable is required'
            forced_command = shlex.join([sys.executable, '-I',
                str(Path(__file__).with_name('drop_hardlink_ack.py').resolve()),
                str(subsystem), str(remote), str(root/'suppressed-ack.json')])
        server_config.write_text(f'''Port {port}
ListenAddress 127.0.0.1
HostKey {root/'host'}
PidFile {root/'sshd.pid'}
AuthorizedKeysFile {root/'client.pub'}
StrictModes no
UsePAM yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
PermitTTY no
DisableForwarding yes
PrintMotd no
LogLevel VERBOSE
Subsystem sftp internal-sftp
ForceCommand {forced_command}
''')
        config = root/'client_config'
        config.write_text(f'''Host fixture
 HostName 127.0.0.1
 Port {port}
 User {getpass.getuser()}
 IdentityFile {root/'client'}
 IdentityAgent none
 IdentitiesOnly yes
 UserKnownHostsFile {known}
 GlobalKnownHostsFile /dev/null
 StrictHostKeyChecking yes
 BatchMode yes
''')
        prefix = ['sudo', '-n'] if options.sudo_sshd else []
        with (root/'sshd.log').open('wb') as log:
            server = subprocess.Popen([*prefix, '/usr/sbin/sshd', '-D', '-e', '-f', str(server_config)], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while True:
                assert server.poll() is None and time.monotonic() < deadline, (root/'sshd.log').read_text()
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.2):
                        break
                except OSError:
                    time.sleep(.05)
            if options.lost_ack:
                args = [sys.executable, str(Path(__file__).with_name('check_lost_ack.py')), '--binary', str(binary),
                        '--config', str(config), '--remote-root', str(remote), '--ack-marker', str(root/'suppressed-ack.json')]
            else:
                args = [sys.executable, str(Path(__file__).with_name('check_transport.py')), '--binary', str(binary),
                        '--config', str(config), '--host', 'fixture', '--remote-root', str(remote), '--local-server-root', str(remote)]
            if options.report:
                args += ['--report', str(options.report)]
            subprocess.run(args, check=True, timeout=120)
            if options.adversarial and not options.lost_ack:
                args = [sys.executable, str(Path(__file__).with_name('check_adversarial.py')), '--binary', str(binary),
                        '--config', str(config), '--remote-root', str(remote)]
                if options.report:
                    args += ['--report', str(options.report.with_name('linux-adversarial.json'))]
                subprocess.run(args, check=True, timeout=150)
            if options.ui_binary:
                assert not options.lost_ack, 'UI requires the plain disposable SFTP server'
                environment = dict(os.environ, SSH_FILES_TEST_CONFIG=str(config), SSH_FILES_TEST_REMOTE=str(remote), SSH_FILES_UI_BINARY=str(options.ui_binary.resolve(strict=True)))
                subprocess.run(['cargo', '+1.94.0', 'test', '--locked', '--test', 'native_pty', 'actual_', '--', '--ignored', '--nocapture', '--test-threads=1'], env=environment, check=True, timeout=150)
                args = [sys.executable, str(Path(__file__).with_name('check_pty_shutdown.py')), '--binary', str(options.ui_binary.resolve()), '--config', str(config), '--remote-root', str(remote)]
                if options.report:
                    args += ['--report', str(options.report.with_name('linux-pty-shutdown.json'))]
                subprocess.run(args, check=True, timeout=90)
        except BaseException:
            print((root/'sshd.log').read_text(errors='replace')[-16000:], file=sys.stderr)
            raise
        finally:
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
        print('PASS: owned Linux sshd stopped; fixture keys/config/files removed on exit', flush=True)


if __name__ == '__main__':
    main()
