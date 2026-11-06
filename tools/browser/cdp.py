"""Bounded synchronous CDP transport for private Chrome reference captures.

Callers must verify browser PID/profile/screen ownership before connecting, and
wait for the owned screen to be free before commands that change its state.
Unsolicited events are discarded; this is not a network recording driver.
"""
import base64
import hashlib
import json
import os
import socket
import struct
import urllib.parse


class Connection:
    limit = 16 << 20

    def __init__(self, url, timeout=30):
        url = urllib.parse.urlsplit(url)
        assert url.scheme == 'ws' and url.hostname in ('127.0.0.1', 'localhost', '::1')
        self.socket = socket.create_connection((url.hostname, url.port), timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        path = url.path + ('?' + url.query if url.query else '')
        self.socket.sendall(f'GET {path} HTTP/1.1\r\nHost: {url.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode())
        header = bytearray()
        while not header.endswith(b'\r\n\r\n'):
            header.extend(self.read(1))
            if len(header) > 16384:
                raise ValueError('Oversized WebSocket handshake')
        expected = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        if not header.startswith(b'HTTP/1.1 101 ') or expected not in header:
            raise ValueError('Invalid WebSocket handshake')
        self.serial = 0

    def read(self, size):
        data = bytearray()
        while len(data) < size:
            part = self.socket.recv(min(size - len(data), 65536))
            if not part:
                raise EOFError('Chrome disconnected')
            data.extend(part)
        return bytes(data)

    def send(self, opcode, data):
        mask = os.urandom(4)
        size = len(data)
        header = bytes([0x80 | opcode, 0x80 | size]) if size < 126 else bytes([0x80 | opcode, 0xfe]) + struct.pack('!H', size) if size < 65536 else bytes([0x80 | opcode, 0xff]) + struct.pack('!Q', size)
        self.socket.sendall(header + mask + bytes(value ^ mask[i % 4] for i, value in enumerate(data)))

    def call(self, method, **params):
        self.serial += 1
        self.send(1, json.dumps(dict(id=self.serial, method=method, params=params)).encode())
        message = bytearray()
        while True:
            flags, size = self.read(2)
            if flags & 0x70 or size & 0x80:
                raise ValueError('Unexpected WebSocket flags')
            size &= 127
            if size == 126:
                size = struct.unpack('!H', self.read(2))[0]
            elif size == 127:
                size = struct.unpack('!Q', self.read(8))[0]
            if size > self.limit - len(message):
                raise ValueError('CDP message exceeds capture budget')
            chunk = self.read(size)
            opcode = flags & 15
            if opcode == 8:
                raise EOFError('Chrome closed WebSocket')
            if opcode == 9:
                self.send(10, chunk)
                continue
            if opcode == 10:
                continue
            if opcode not in (0, 1):
                raise ValueError('Unexpected WebSocket opcode')
            message.extend(chunk)
            if not flags & 128:
                continue
            reply = json.loads(message)
            message.clear()
            if reply.get('id') != self.serial:
                continue
            if 'error' in reply:
                raise RuntimeError(reply['error'])
            return reply['result']

    def close(self):
        self.socket.close()
