#!/usr/bin/python3
"""A slow link, for tests: a unix socket that forwards to another, each way after a delay, in order.

    slowlink.py LISTEN UPSTREAM DELAY_MS [JITTER_MS [STALL_PERCENT STALL_MS]]

Each chunk read is delivered DELAY_MS (+ up to JITTER_MS) later, never before an earlier one of its direction (as
TCP: a late packet holds those behind it). With STALL_PERCENT, that share of chunks is held STALL_MS more (a packet
lost and waited for), and all behind it with it. Prints nothing; ends when killed.

With REX_SLOW_HELPER (a command, as a JSON list) the control connection goes to that command's input and output
instead of UPSTREAM: Olo's helper on a host (olo-rex-link), which runs the Rex server's own dial-stdio behind it,
as over SSH. Every other connection goes to UPSTREAM as before.

LISTEN and UPSTREAM may each be `tcp:PORT` (on 127.0.0.1) instead of a socket's path: a slow link in front of an SSH
server (farhost.py's). With REX_SLOW_COUNT (a file), the bytes that passed each way and the connections made are
written there as JSON ({"up": …, "down": …, "connections": …}: up is from the side that connects), at most ten
times a second while they change: what a connection costs on the wire.

With REX_SLOW_RATE (bytes a second, each way) the link carries no more than that: what is read waits its turn, and
no more than REX_SLOW_QUEUE bytes (64,000 unless said: a router's queue) wait at once; past that nothing more is
read from the sender, whose own buffers then fill, as on a link that is narrow and not only far. A flood then holds
up what comes behind it on the same connection, which a delay alone never shows.

With REX_SLOW_TRACE (a file), every chunk read is noted there as it is read, before its delay: the time in ms since
the link started, which connection, which way (> from the side that connects), its size and how it begins (a
WebSocket frame's text where it is one whole unmasked frame, else the bytes as text): the order of what an app
asks and is told, to count the round trips of something (connecting)."""
import asyncio, json, os, random, sys

LISTEN, UPSTREAM, DELAY = sys.argv[1], sys.argv[2], float(sys.argv[3]) / 1000
JITTER = float(sys.argv[4]) / 1000 if len(sys.argv) > 4 else 0.0
STALL_P = float(sys.argv[5]) / 100 if len(sys.argv) > 5 else 0.0
STALL = float(sys.argv[6]) / 1000 if len(sys.argv) > 6 else 0.0
HELPER = json.loads(os.environ.get("REX_SLOW_HELPER") or "null")
COUNT = os.environ.get("REX_SLOW_COUNT")
RATE = float(os.environ.get("REX_SLOW_RATE") or 0)
QUEUE = int(os.environ.get("REX_SLOW_QUEUE") or 64000)
TRACE = os.environ.get("REX_SLOW_TRACE")
trace = open(TRACE, "w", buffering=1) if TRACE else None
began = None

def noted(n, way, data):
    """A line of the trace for a chunk."""
    text = data
    # (A frame from a server isn't masked: its text follows its header. One from a client is under its mask,
    # which is taken off.)
    if len(data) > 2 and data[0] in (0x81, 0x82):
        size = data[1] & 0x7f; at = 2 + (2 if size == 126 else 8 if size == 127 else 0)
        if data[1] & 0x80:
            key = data[at:at + 4]; at += 4
            text = bytes(b ^ key[i % 4] for i, b in enumerate(data[at:at + 160]))
        else:
            text = data[at:at + 160]
    shown = text[:160].decode("utf-8", "replace").replace("\r", " ").replace("\n", " ")
    trace.write(f"{(asyncio.get_running_loop().time() - began) * 1000:8.0f} ms  #{n} {'>' if way == 'up' else '<'} {len(data):6}  {shown}\n")
passed = {"up": 0, "down": 0, "connections": 0}
dirty = False

async def counted():
    global dirty
    while True:
        await asyncio.sleep(0.1)
        if dirty:
            dirty = False
            with open(COUNT + ".new", "w") as f:
                json.dump(passed, f)
            os.replace(COUNT + ".new", COUNT)

def held():
    return DELAY + random.random() * JITTER + (STALL if random.random() < STALL_P else 0.0)

async def pump(reader, writer, first=None, way="down", n=0):
    global dirty
    loop = asyncio.get_running_loop()
    queue = asyncio.Queue()
    if first:
        if trace: noted(n, way, first)
        passed[way] += len(first); dirty = True
        queue.put_nowait((loop.time() + held(), first))
    waiting = 0                     # bytes read and not yet delivered
    room = asyncio.Event(); room.set()
    async def deliver():
        nonlocal waiting
        last = 0.0
        free = 0.0                  # when the link is next free to carry (a narrow link's own queue)
        while True:
            at, data = await queue.get()
            at = max(at, last)          # in order
            last = at
            wait = at - loop.time()
            if wait > 0:
                await asyncio.sleep(wait)
            if data is None:
                try:
                    writer.close()
                except Exception:
                    pass
                return
            try:
                if RATE:
                    # (In pieces of a twentieth of a second's worth: each leaves when the one before has.)
                    piece = max(1, int(RATE / 20))
                    for i in range(0, len(data), piece):
                        part = data[i:i + piece]
                        free = max(free, loop.time()) + len(part) / RATE
                        pause = free - loop.time()
                        if pause > 0:
                            await asyncio.sleep(pause)
                        writer.write(part)
                        waiting -= len(part)
                        if waiting <= QUEUE: room.set()
                    await writer.drain()
                else:
                    writer.write(data)
                    await writer.drain()
            except Exception:
                return
    task = asyncio.create_task(deliver())
    try:
        while True:
            if RATE:
                # (A narrow link's queue is full: nothing more is read, and the sender's buffers fill.)
                await room.wait()
            data = await reader.read(65536 if not RATE else max(1024, min(65536, QUEUE // 4)))
            if trace and data: noted(n, way, data)
            passed[way] += len(data); dirty = True
            if RATE and data:
                waiting += len(data)
                if waiting > QUEUE: room.clear()
            await queue.put((loop.time() + held(), data or None))
            if not data:
                break
    except Exception:
        await queue.put((loop.time(), None))
    await task

async def serve(reader, writer):
    try:
        # (What it asks for is in its first bytes: the upgrade's request line.)
        first = await reader.read(65536)
        if HELPER and b" /api/control " in first.split(b"\r\n", 1)[0]:
            helper = await asyncio.create_subprocess_exec(*HELPER, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)
            up_reader, up_writer = helper.stdout, helper.stdin
        elif UPSTREAM.startswith("tcp:"):
            up_reader, up_writer = await asyncio.open_connection("127.0.0.1", int(UPSTREAM[4:]))
        else:
            up_reader, up_writer = await asyncio.open_unix_connection(UPSTREAM)
    except Exception:
        writer.close()
        return
    passed["connections"] += 1
    n = passed["connections"]
    await asyncio.gather(pump(reader, up_writer, first, "up", n), pump(up_reader, writer, None, "down", n))

async def main():
    global began
    began = asyncio.get_running_loop().time()
    if COUNT:
        asyncio.create_task(counted())
    if LISTEN.startswith("tcp:"):
        server = await asyncio.start_server(serve, "127.0.0.1", int(LISTEN[4:]))
    else:
        if os.path.exists(LISTEN):
            os.remove(LISTEN)
        server = await asyncio.start_unix_server(serve, LISTEN)
    async with server:
        await server.serve_forever()

asyncio.run(main())
