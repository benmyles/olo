# Olo Browser development shell

`olo-browser [about:engine | /absolute/path/to/file.html]` opens the original
Rust page engine in a native `olo-ui` window. It remains a development browser;
Chrome compatibility has not passed the acceptance matrix. HTTP/HTTPS,
linked CSS, links, Back/Forward, images, V8 scripting and parts of the modern
DOM, layout, canvas and media APIs work. Complete standards behavior,
tabs, complete image/media support, sandboxing,
password/passkey mediation and full Olo integrations are still required.
It does not change the installed/default browser.

The detailed sections below originated as static-pipeline milestones. For
current implementation work and remaining failures, consult
`docs/browser/JOURNAL.md` and `docs/browser/TODO.md`; a component test passing
does not establish website or production readiness.

HTML parsing, style, font work, layout, scene construction, tile rasterization,
file reads and image reclamation run on a worker. Progress streams through
the kit proxy; Stop/Escape cancel work. Only viewport tiles are uploaded;
the worker retains up to 256 256×256 BGRA tiles (64 MiB payload). Outlines,
text shaping and width-independent text geometry have separate bounded
caches. Tiles are keyed by page/style/scale/viewport and copied pixel-for-pixel
to GPU textures. This is CPU outline rasterization plus GPU composition,
not a complete GPU web renderer. File input currently accepts UTF-8 HTML,
up to 16 MiB; embedded styles up to 4 MiB.

PNG/JPEG/WebP/GIF stills load from HTTP/HTTPS, allowed local files and data
URLs, with 16 MiB encoded input per image and a 64 MiB aggregate decoded
pixel budget, separate from retained tiles. Repeated canonical image URLs
share one decode per navigation, and resize/theme restyles reuse those assets.
Codec allocation hints are best effort; isolation and interruptible codec CPU
work remain open. Broken-image fallback, responsive sources, animation, SVG,
complete alignment and GPU sampling remain incomplete. Minification uses lazy,
cancellable source-pixel mip levels with a separate 32 MiB / 128-source LRU
budget. Resize/theme changes reuse levels; navigation releases them. The pinned
Chrome pixel controls match 53/60 captures exactly, with seven explicit nonpasses;
this is component coverage, not website compatibility.

The page canvas, address field, Go/Stop buttons, visible text, progress/errors
and menus use the shared structured agent tree and menu API. For example,
on an owned Agent Screen:

```
olo-screen run OWN_SCREEN /absolute/build/path/olo-browser
olo-screen set OWN_SCREEN --role TextInput --name Address /var/tmp/OWNED/page.html
olo-screen key OWN_SCREEN Enter
olo-screen tree OWN_SCREEN --json
olo-screen invoke OWN_SCREEN reload
olo-screen invoke OWN_SCREEN stop
```

Cmd+L selects the address, Cmd+R reloads, Escape stops. Menu actions use the
same command handlers as people. Forwarded launches replace the document in
the existing window; relative paths resolve against the sender's working
directory. Page and CSS scroll position survive kit re-execution/reconnection.
Document snapshots include visible paint-run text, image alt text/bounds and
link actions through the same input dispatch as people. Linked images are
Link records; other images are Image records. Complete semantic DOM snapshots,
selection, editing and input/IME remain required. Pixel/source bytes are not
included in the structured tree.

Native Canvas/CanvasText/system colours follow the palette. This does not
adapt author-defined light colours into universal dark/night mode. See
`docs/browser` for the active acceptance criteria, journal and TODO.
