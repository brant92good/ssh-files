import select
import socket
import threading
import time

class TransportGate:
    """Only closes this fixture's transport, including already connected sockets."""

    def __init__(self, target):
        self.target = target
        self.enabled = True
        self.paused = threading.Event()
        self.closed = threading.Event()
        self.lock = threading.Lock()
        self.connections = set()
        self.workers = []
        self.accepted = 0
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen()
        self.listener.settimeout(.2)
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.accept)
        self.thread.start()

    def accept(self):
        while not self.closed.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with self.lock:
                self.accepted += 1
                worker = threading.Thread(target=self.relay, args=(client,))
                self.workers.append(worker)
                worker.start()

    def relay(self, client):
        remote = None
        try:
            with self.lock:
                if not self.enabled or self.closed.is_set():
                    return
                self.connections.add(client)
            remote = socket.create_connection(self.target, timeout=2)
            with self.lock:
                if not self.enabled or self.closed.is_set():
                    return
                self.connections.add(remote)
            while not self.closed.is_set():
                if self.paused.is_set():
                    time.sleep(.01)
                    continue
                ready, _, _ = select.select([client, remote], [], [], .2)
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    (remote if source is client else client).sendall(data)
        except (OSError, ValueError):
            pass
        finally:
            with self.lock:
                self.connections.discard(client)
                self.connections.discard(remote)
            client.close()
            if remote is not None:
                remote.close()

    def offline(self):
        with self.lock:
            self.enabled = False
            for connection in tuple(self.connections):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                connection.close()

    def online(self):
        with self.lock:
            self.enabled = True

    def count(self):
        with self.lock:
            return self.accepted

    def close(self):
        self.closed.set()
        self.offline()
        self.listener.close()
        self.thread.join(timeout=3)
        for worker in self.workers:
            worker.join(timeout=3)
        assert not self.thread.is_alive() and not any(w.is_alive() for w in self.workers)

