#!/usr/bin/python3
"""A slow link, for tests: a unix socket that forwards to another, each way after a delay, in order.

    slowlink.py LISTEN UPSTREAM DELAY_MS [JITTER_MS [STALL_PERCENT STALL_MS]]

Each chunk read is delivered DELAY_MS (+ up to JITTER_MS) later, never before an earlier one of its direction (as
TCP: a late packet holds those behind it). With STALL_PERCENT, that share of chunks is held STALL_MS more (a packet
lost and waited for), and all behind it with it. Prints nothing; ends when killed."""
import asyncio, os, random, sys

LISTEN, UPSTREAM, DELAY = sys.argv[1], sys.argv[2], float(sys.argv[3]) / 1000
JITTER = float(sys.argv[4]) / 1000 if len(sys.argv) > 4 else 0.0
STALL_P = float(sys.argv[5]) / 100 if len(sys.argv) > 5 else 0.0
STALL = float(sys.argv[6]) / 1000 if len(sys.argv) > 6 else 0.0

async def pump(reader, writer):
    loop = asyncio.get_running_loop()
    queue = asyncio.Queue()
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
            hold = DELAY + random.random() * JITTER + (STALL if random.random() < STALL_P else 0.0)
            await queue.put((loop.time() + hold, data or None))
            if not data:
                break
    except Exception:
        await queue.put((loop.time(), None))
    await task

async def serve(reader, writer):
    try:
        up_reader, up_writer = await asyncio.open_unix_connection(UPSTREAM)
    except Exception:
        writer.close()
        return
    await asyncio.gather(pump(reader, up_writer), pump(up_reader, writer))

async def main():
    if os.path.exists(LISTEN):
        os.remove(LISTEN)
    server = await asyncio.start_unix_server(serve, LISTEN)
    async with server:
        await server.serve_forever()

asyncio.run(main())
