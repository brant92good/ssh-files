"""Test-only transparent relay to a real sftp-server; drop one successful link ACK.

All SFTP operations execute in the official server. The relay only recognizes
the hardlink extension request ID and withholds its successful STATUS packet.
Other packets pass through unchanged. This is fault injection, not a backend.
"""
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import threading

server_path, remote, marker = sys.argv[1:]
def trace(text):
    with Path(marker + '.relay.log').open('a', encoding='utf-8') as log:
        log.write(text + '\n')
trace('relay started')
trace("OpenSSH environment keys: " + repr([k for k in os.environ if "OPENSSH" in k or "POSIX" in k]))
# A Windows OpenSSH grandparent exports its own CRT fd table. Our child
# receives new synchronous Python pipes, so inheriting that table would install
# stale HANDLE values (official w32fd.c explicitly documents this case).
child_env = dict(os.environ)
if os.name == 'nt':
    for key in list(child_env):
        if key.endswith("_POSIX_FD_STATE"):
            trace("removed fd state key " + key)
            child_env.pop(key)
    child_env["OPENSSH_STDIO_MODE"] = "nonsock_sync"
server_log = Path(marker + '.server.log').open('wb')
server = subprocess.Popen([server_path, "-d", remote, "-e", "-l", "DEBUG3"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=server_log,
                          creationflags=0x08000000 if os.name == 'nt' else 0,
                          cwd=remote, bufsize=0, env=child_env)
pending = set()
guard = threading.Lock()

def packet(stream):
    def exact(size):
        parts = bytearray()
        while len(parts) < size:
            chunk = stream.read(size - len(parts))
            if not chunk:
                break
            parts.extend(chunk)
        return bytes(parts)
    header = exact(4)
    if not header:
        return None
    if len(header) != 4:
        raise ValueError("truncated fixture packet header")
    size = struct.unpack(">I", header)[0]
    if size == 0 or size > 4 * 1024 * 1024:
        raise ValueError("invalid fixture packet size")
    body = exact(size)
    if len(body) != size:
        raise ValueError("truncated fixture packet")
    return header + body

def responses():
    try:
        while (value := packet(server.stdout)) is not None:
            body = value[4:]
            trace(f'reply type {body[0]} bytes {len(body)}')
            if len(body) >= 9 and body[0] == 101:
                request_id, status = struct.unpack(">II", body[1:9])
                with guard:
                    suppress = request_id in pending and status == 0
                    pending.discard(request_id)
                if suppress:
                    Path(marker).write_text(json.dumps({"request_id": request_id, "status": status,
                        "event": "actual-server-successful-hardlink-status-suppressed",
                        "relay_pid": os.getpid(), "server_pid": server.pid}), encoding="utf-8")
                    continue
            sys.stdout.buffer.write(value)
            sys.stdout.buffer.flush()
        trace(f'server stdout EOF, exit={server.poll()}')
    except (BrokenPipeError, OSError):
        pass

worker = threading.Thread(target=responses, daemon=True)
worker.start()
try:
    # Avoid a buffered stdin reader requesting an oversized read from sshd's
    # Windows pipe while a small SFTP packet is waiting for its response.
    while (value := packet(sys.stdin.buffer.raw)) is not None:
        body = value[4:]
        trace(f'request type {body[0]} bytes {len(body)}')
        if len(body) >= 9 and body[0] == 200:
            request_id, length = struct.unpack(">II", body[1:9])
            if body[9:9 + length] == b"hardlink@openssh.com":
                with guard:
                    pending.add(request_id)
        remaining = memoryview(value)
        while remaining:
            count = server.stdin.write(remaining)
            if not count:
                raise BrokenPipeError('Actual SFTP server accepted no bytes')
            remaining = remaining[count:]
        trace(f'forwarded {len(value)} bytes, server exit={server.poll()}')
        server.stdin.flush()
finally:
    server.stdin.close()
    try:
        server.wait(timeout=3)
    except subprocess.TimeoutExpired:
        server.kill()
        server.wait(timeout=3)
    worker.join(timeout=2)
