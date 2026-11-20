# Token Pool's TLS handshake, beside the clients': what differs, and what matching would take

Ben, 2026-11-19: "it should send exactly what claude code would send so
that it's transparent to the server that it's going through the pool";
"it must be identical to the client"; "all requests from the pool need
to be exactly the same as if they came from claude code / codex".

The pool's requests are now the clients' byte for byte from the first
line of the request on (HACKING, "Token Pool"). Under them is the TLS
handshake, whose first message (the ClientHello) a host sees before any
request: which cipher suites, signature schemes and extensions the
program offers, in what order. Hosts and the networks in front of them
(Cloudflare, for both providers) reduce it to a fingerprint (JA3, JA4)
and can tell one TLS library from another by it. This is what the pool
still sends as itself. TODO asked for it weighed, with the captures as
the reference. 2026-11-20.

## What each client offers

From `Capture::hellos` (every handshake of every run of a capture, as
the recorder's acceptor read it: ALPN, cipher suites and signature
schemes, in the client's order), Claude Code 2.1.329 and Codex 0.170.0,
the same in every account's capture.

**Claude Code** (a Bun program: BoringSSL) has two, both to
api.anthropic.com:

| | its SDK's (Bun's `fetch`: `/v1/messages`) | its API client's (`node:https`: `/api/*`, renewal) |
|---|---|---|
| ALPN | `http/1.1` | none |
| Suites | 17: the three of TLS 1.3 (AES-128-GCM first), then ECDHE with ECDSA before RSA (AES-128-GCM, AES-256-GCM, ChaCha20), the CBC-SHA four, and RSA key exchange's four (0x9c, 0x9d, 0x2f, 0x35) | 18: the same three, then ECDHE with RSA before ECDSA, with ECDHE-RSA-AES128-CBC-SHA256 among them (Node's default list) |
| Signature schemes | 11: ECDSA P-256, RSA-PSS and PKCS#1 with SHA-256, the same with 384, with 512, Ed25519, ECDSA P-521, and RSA PKCS#1 SHA-1 last | the same 11 |

**Codex** (a Rust program) has two as well, but which one a host sees
depends on how Codex is run:

| | with only its own roots (run `native`: how it comes) | trusting an authority of the person's (every other run) |
|---|---|---|
| Library | OpenSSL 3 (the system's, through native-tls) | rustls with aws-lc-rs |
| ALPN | none (so HTTP/1.1) | `h2`, `http/1.1`; none on its other connections (its WebSocket's) |
| Suites | 30: OpenSSL's default list (TLS 1.3 with AES-256-GCM first, then ECDHE and DHE with GCM, ChaCha20, CBC…) | 10: TLS 1.3's three (AES-256-GCM first), ECDHE with ECDSA then RSA (AES-256-GCM, AES-128-GCM, ChaCha20), and the renegotiation SCSV |
| Signature schemes | 26: ML-DSA's three first, ECDSA, Ed25519, Ed448, brainpool, RSA-PSS (both kinds), PKCS#1 down to SHA-1 and SHA-224 | 13: ECDSA P-384, P-256, P-521, Ed25519, RSA-PSS 512/384/256, PKCS#1 512/384/256, ML-DSA 44/65/87 |

(Codex takes rustls when it's given roots to trust beside its own, as
a company's proxy has people do, and as the pool's does: so the leg
from Codex to the pool is always the right-hand one.)

## What the pool offers

One offer for every host: rustls 0.23 with aws-lc-rs
(`relay::tls_offer`), the ALPN the client offered the pool. Its suites
and schemes, in order, are exactly Codex's right-hand column: the same
library, of the same generation (a test holds it:
`relay::more::the_pools_offer_is_codexs_under_an_authority`).

So, to ChatGPT, a request through the pool has the handshake of a
Codex that trusts a company's authority: one of Codex's own two, and
the one Codex is in fact using when it runs through the pool. To
Anthropic, a request through the pool has neither of Claude Code's
handshakes: ten suites where Bun offers seventeen or eighteen, in
another order, other schemes (ML-DSA, no SHA-1), and rustls's
extensions. Anything that keys on the handshake sees "not Bun" there,
whatever the request under it says.

What the captures don't record is the rest of the ClientHello:
extensions and their order, the groups and key shares, session
tickets, GREASE. rustls's acceptor gives the recorder suites, schemes,
ALPN and the name; a byte-for-byte comparison needs the record itself.

## What matching would take

**Claude Code's two (BoringSSL).** The `boring` crate (Cloudflare's
bindings; `tokio-boring` for the stream) builds BoringSSL from source:
cmake and a C++ compiler at build time, a minute or two cold (kept by
kache after), and a second TLS library in the pool's programs (about
3 MB, and its own security updates to follow). The relay already
drives hyper over any stream, so the change is its connector: for
Anthropic's hosts, a BoringSSL connection set up as Bun sets its up
(the TLS 1.2 list in each stack's order, the signature algorithms, the
groups, the ALPN or none), one profile for each of Claude Code's two
stacks, chosen by which of them sent the request (the relay knows: the
client's own offer to the pool, `ClientOffer`, `http/1.1` or none).
BoringSSL fixes TLS 1.3's suite order and its extension order itself,
and Bun doesn't permute them, so the same library with the same lists
should give the same message; "should" until it's compared. Not
knowable without building it: whether the BoringSSL the crate carries
and the one in the installed Bun differ in a default that shows (a
group added, a key share), and whether each such default has a switch.

**Codex's other one (OpenSSL).** The `openssl` crate against the
system's library, as Codex's native-tls uses it, with no ALPN and so
HTTP/1.1 to ChatGPT. It would trade the handshake of one real Codex
for the handshake of another: no gain in likeness, and a loss (the
pool's own leg would no longer be the stack the Codex behind it is
using). Not worth doing.

## What I'd do, and in what order

1. **Nothing for Codex.** The pool's handshake is already one of
   Codex's own, and the test now says so if rustls ever drifts from
   it.
2. **Record the whole ClientHello in captures** before deciding the
   rest: the recorder keeps the first record's bytes beside what it
   keeps now (the random, session id and key shares aside), for each
   stack. No new dependency, and it turns "should match" into a diff.
   A day's work, with the recorder's tests.
3. **Then BoringSSL for Anthropic's hosts, if likeness at this layer
   matters to you**, checked the same way the Codex test checks rustls
   (the pool's offer read by a local acceptor, compared with the
   capture's record). It is the one place left where the pool is
   plainly not the client. The cost is a C++ library built with Olo
   and kept current in a program that holds sign-ins; the gain is that
   nothing a host sees before or in a request tells the pool from
   Claude Code. I'd do it: the rest of the pool's care (requests byte
   for byte, each account's own connections, the client's own cookies)
   is spent on a likeness the handshake gives away first.

Step 3 is a decision of yours (a new kind of dependency, in the
program that holds the accounts): it waits in TODO for a yes.
