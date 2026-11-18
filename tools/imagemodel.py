#!/usr/bin/env python3
"""OpenAI's image model, as Olo's picture tools call it.

What the wallpaper tools share (genwallpaper.py, upscalewallpaper.py):
the model and quality every picture is made with, the key, and a
multipart body for the images/edits endpoint. (Olo's icons are no longer
pictures from a model: they're drawings, docs/ICONS.md.)

Reads OPENAI_API_KEY from the environment or ~ben/.env. Never print it.
"""
import os, sys, uuid

MODEL = "gpt-image-2.6-sunburst"
QUALITY = "high"


def key():
    k = os.environ.get("OPENAI_API_KEY")
    if k:
        return k
    for line in open(os.path.expanduser("~ben/.env")):
        if line.startswith("OPENAI_API_KEY="):
            return line.split("=", 1)[1].strip().strip('"').strip("'")
    sys.exit("no OPENAI_API_KEY")


def multipart(fields, files):
    boundary = uuid.uuid4().hex
    parts = []
    for k, v in fields.items():
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode())
    for k, path in files:
        data = open(path, "rb").read()
        head = (f'--{boundary}\r\nContent-Disposition: form-data; name="{k}"; '
                f'filename="{os.path.basename(path)}"\r\nContent-Type: image/png\r\n\r\n')
        parts.append(head.encode() + data + b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    return b"".join(parts), f"multipart/form-data; boundary={boundary}"
