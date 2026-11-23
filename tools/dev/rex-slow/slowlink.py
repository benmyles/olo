#!/usr/bin/python3
"""A slow link, for tests: a unix socket that forwards to another, each way after a delay, in order.

    slowlink.py LISTEN UPSTREAM DELAY_MS [JITTER_MS [STALL_PERCENT STALL_MS]]

Each chunk read is delivered DELAY_MS (+ up to JITTER_MS) later, never before an earlier one of its direction (as
TCP: a late packet holds those behind it). With STALL_PERCENT, that share of chunks is held STALL_MS more (a packet
lost and waited for), and all behind it with it. Prints nothing; ends when killed.

With REX_SLOW_HELPER (a command, as a JSON list) the control connection goes to that command's input and output
instead of UPSTREAM: Olo's helper on a host (olo-rex-link), which runs the Rex server's own dial-stdio behind it,
as over SSH. Every other connection goes to UPSTREAM as before."""
import asyncio, json, os, random, sys

LISTEN, UPSTREAM, DELAY = sys.argv[1], sys.argv[2], float(sys.argv[3]) / 1000
JITTER = float(sys.argv[4]) / 1000 if len(sys.argv) > 4 else 0.0
STALL_P = float(sys.argv[5]) / 100 if len(sys.argv) > 5 else 0.0
STALL = float(sys.argv[6]) / 1000 if len(sys.argv) > 6 else 0.0
HELPER = json.loads(os.environ.get("REX_SLOW_HELPER") or "null")

def held():
    return DELAY + random.random() * JITTER + (STALL if random.random() < STALL_P else 0.0)

async def pump(reader, writer, first=None):
    loop = asyncio.get_running_loop()
    queue = asyncio.Queue()
    if first:
        queue.put_nowait((loop.time() + held(), first))
    async def deliver():
        last = 0.0
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
                writer.write(data)
                await writer.drain()
            except Exception:
                return
    task = asyncio.create_task(deliver())
    try:
        while True:
            data = await reader.read(65536)
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
        else:
            up_reader, up_writer = await asyncio.open_unix_connection(UPSTREAM)
    except Exception:
        writer.close()
        return
    await asyncio.gather(pump(reader, up_writer, first), pump(up_reader, writer))

async def main():
    if os.path.exists(LISTEN):
        os.remove(LISTEN)
    server = await asyncio.start_unix_server(serve, LISTEN)
    async with server:
        await server.serve_forever()

asyncio.run(main())
