# Olo Browser development shell

`olo-browser [about:engine | /absolute/path/to/file.html]` opens the original
Rust static page pipeline in a native `olo-ui` window. It is an initial
single-document shell, not a Chrome-compatible browser. Network navigation,
JavaScript, complete CSS/layout, input/hit testing and links, tabs/history,
images/media, sandboxing, password/passkey mediation and full Olo integrations
are still required. It does not change the installed/default browser.

HTML parsing, style, font work, layout, scene construction, tile rasterization,
file reads and image reclamation run on a worker. Progress streams through
the kit proxy; Stop/Escape cancel work. Only viewport tiles are uploaded;
the worker retains up to 256 256×256 BGRA tiles (64 MiB payload). Outlines,
text shaping and width-independent text geometry have separate bounded
caches. Tiles are keyed by page/style/scale/viewport and copied pixel-for-pixel
to GPU textures. This is CPU outline rasterization plus GPU composition,
not a complete GPU web renderer. File input currently accepts UTF-8 HTML,
up to 16 MiB; embedded styles up to 4 MiB. File URLs lack full URL decoding.

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
Document text snapshots are initial visible paint-run labels; semantic DOM
snapshots, stable action identities, selection, input/IME and links are not
implemented. They must not be inferred from this readable text surface.

Native Canvas/CanvasText/system colours follow the palette. This does not
adapt author-defined light colours into universal dark/night mode. See
`docs/browser` for the active acceptance criteria, journal and TODO.
