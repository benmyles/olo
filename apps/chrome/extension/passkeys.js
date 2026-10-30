// In the page's own world, in every frame, before the page's scripts:
// Olo's Passwords as the page's passkey authenticator. A site asking to
// make a passkey is offered Passwords' (the person agrees in Passwords);
// a site asking for one gets Passwords' when it keeps one for the site,
// and Chrome's own way (a phone by its QR code, a security key, Google's)
// otherwise, or when the person chooses "Use Another Device…". A sign-in
// that offers passkeys as the account field's suggestions (conditional
// mediation) has them in the field's context menu, beside Chrome's own.
// Everything goes through relay.js to the extension (background.js),
// which builds the client data from the frame's real origin.

(() => {
  if (!window.PublicKeyCredential || !window.CredentialsContainer || window.__oloPasskeys) return;
  Object.defineProperty(window, "__oloPasskeys", { value: true });

  const container = CredentialsContainer.prototype;
  const nativeCreate = container.create;
  const nativeGet = container.get;
  const PKC = PublicKeyCredential;

  // ---- Bytes as base64url, as WebAuthn's JSON forms have them ----

  function bytes(s) {
    if (s instanceof ArrayBuffer) return new Uint8Array(s);
    if (ArrayBuffer.isView(s)) return new Uint8Array(s.buffer, s.byteOffset, s.byteLength);
    return new Uint8Array(0);
  }

  function b64(s) {
    let out = "";
    for (const b of bytes(s)) out += String.fromCharCode(b);
    return btoa(out).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function unb64(s) {
    const t = s.replace(/-/g, "+").replace(/_/g, "/");
    const bin = atob(t + "===".slice((t.length + 3) % 4));
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out.buffer;
  }

  // ---- Talking to the extension (relay.js) ----

  // A request's answers by its id: the first, and for a conditional one,
  // the passkey chosen in the menu later.
  const answers = new Map();
  let next = 1;

  addEventListener("message", (e) => {
    if (e.source !== window || e.data?.__oloPasskey !== "answer") return;
    answers.get(e.data.rid)?.(e.data);
  });

  function post(m) {
    window.postMessage({ __oloPasskey: true, ...m }, "*");
  }

  // Send `request`; `listen` gets each answer.
  function ask(request, listen) {
    const rid = `${next++}-${Math.random().toString(36).slice(2)}`;
    answers.set(rid, listen);
    post({ kind: "request", rid, request });
    return rid;
  }

  function forget(rid) {
    answers.delete(rid);
    post({ kind: "cancel", rid });
  }

  const aborted = (signal) => signal.reason ?? new DOMException("The operation was aborted.", "AbortError");

  // One answer, or the page giving up (`signal`).
  function asked(request, signal) {
    return new Promise((resolve, reject) => {
      if (signal?.aborted) return reject(aborted(signal));
      const rid = ask(request, (a) => {
        answers.delete(rid);
        signal?.removeEventListener("abort", stop);
        resolve(a);
      });
      const stop = () => {
        forget(rid);
        reject(aborted(signal));
      };
      signal?.addEventListener("abort", stop, { once: true });
    });
  }

  // The frame may use WebAuthn (an iframe must be let).
  function allowed(feature) {
    try {
      return !document.featurePolicy || document.featurePolicy.allowsFeature(feature);
    } catch {
      return true;
    }
  }

  // ---- Credentials as the page expects them ----

  // Own properties over the prototype's (whose getters only work on
  // Chrome's own objects), so `instanceof` holds too.
  function define(o, props) {
    for (const [k, v] of Object.entries(props)) Object.defineProperty(o, k, { value: v, enumerable: typeof v !== "function" });
    return o;
  }

  function credential(id, response, extensions) {
    const rawId = unb64(id);
    return define(Object.create(PKC.prototype), {
      id,
      rawId,
      type: "public-key",
      authenticatorAttachment: "platform",
      response,
      getClientExtensionResults: () => ({ ...extensions }),
      toJSON: () => ({ id, rawId: id, type: "public-key", authenticatorAttachment: "platform", response: response.toJSON(), clientExtensionResults: { ...extensions } }),
    });
  }

  const TRANSPORTS = ["hybrid", "internal"];

  function attestation(a, extensions) {
    const [clientDataJSON, attestationObject, authData, publicKey] = [a.clientDataJSON, a.attestationObject, a.authenticatorData, a.publicKey].map(unb64);
    const response = define(Object.create(AuthenticatorAttestationResponse.prototype), {
      clientDataJSON,
      attestationObject,
      getTransports: () => [...TRANSPORTS],
      getAuthenticatorData: () => authData.slice(0),
      getPublicKey: () => publicKey.slice(0),
      getPublicKeyAlgorithm: () => -7,
      toJSON: () => ({
        clientDataJSON: a.clientDataJSON,
        attestationObject: a.attestationObject,
        authenticatorData: a.authenticatorData,
        publicKey: a.publicKey,
        publicKeyAlgorithm: -7,
        transports: [...TRANSPORTS],
      }),
    });
    return credential(a.credentialId, response, extensions);
  }

  function assertion(a) {
    const response = define(Object.create(AuthenticatorAssertionResponse.prototype), {
      clientDataJSON: unb64(a.clientDataJSON),
      authenticatorData: unb64(a.authenticatorData),
      signature: unb64(a.signature),
      userHandle: a.userHandle ? unb64(a.userHandle) : null,
      toJSON: () => ({ clientDataJSON: a.clientDataJSON, authenticatorData: a.authenticatorData, signature: a.signature, userHandle: a.userHandle || undefined }),
    });
    return credential(a.credentialId, response, {});
  }

  function failed(a) {
    return new DOMException(a.message || "The operation either timed out or was not allowed.", a.error || "NotAllowedError");
  }

  // ---- create() and get() ----

  async function create(options) {
    const pk = options?.publicKey;
    const algs = (pk?.pubKeyCredParams || []).map((p) => p.alg);
    const attachment = pk?.authenticatorSelection?.authenticatorAttachment;
    // Only ES256 keys, made here.
    if (!pk || !allowed("publickey-credentials-create") || (algs.length && !algs.includes(-7)) || attachment === "cross-platform") {
      return nativeCreate.call(navigator.credentials, options);
    }
    const a = await asked(
      {
        kind: "create",
        rpId: pk.rp?.id || location.hostname,
        rpName: pk.rp?.name || "",
        challenge: b64(pk.challenge),
        userId: b64(pk.user?.id),
        userName: pk.user?.name || "",
        displayName: pk.user?.displayName || "",
        exclude: (pk.excludeCredentials || []).map((c) => b64(c.id)),
      },
      options.signal,
    );
    if (a.native) return nativeCreate.call(navigator.credentials, options);
    if (a.error) throw failed(a);
    return attestation(a, pk.extensions?.credProps ? { credProps: { rk: true } } : {});
  }

  async function get(options) {
    const pk = options?.publicKey;
    if (!pk || !allowed("publickey-credentials-get")) return nativeGet.call(navigator.credentials, options);
    const request = {
      kind: "get",
      rpId: pk.rpId || location.hostname,
      challenge: b64(pk.challenge),
      allow: (pk.allowCredentials || []).map((c) => b64(c.id)),
    };
    if (options.mediation === "conditional") return conditional(options, request);
    const a = await asked(request, options.signal);
    if (a.native) return nativeGet.call(navigator.credentials, options);
    if (a.error) throw failed(a);
    return assertion(a);
  }

  // A sign-in's passkeys offered as suggestions: Chrome's own (its
  // suggestions under the field) and Passwords' (the field's context
  // menu) at once; whichever is chosen first, the other is let go.
  function conditional(options, request) {
    return new Promise((resolve, reject) => {
      const signal = options.signal;
      if (signal?.aborted) return reject(aborted(signal));
      const theirs = new AbortController();
      let ours = true, done = false;
      const finish = (f) => {
        if (done) return;
        done = true;
        signal?.removeEventListener("abort", stop);
        f();
      };
      const rid = ask({ ...request, conditional: true }, (a) => {
        if (a.native || a.error) ours = false;
        else if (a.credentialId) {
          answers.delete(rid);
          theirs.abort();
          finish(() => resolve(assertion(a)));
        }
      });
      const stop = () => {
        forget(rid);
        theirs.abort();
        finish(() => reject(aborted(signal)));
      };
      signal?.addEventListener("abort", stop, { once: true });
      nativeGet.call(navigator.credentials, { ...options, signal: theirs.signal }).then(
        (c) => {
          forget(rid);
          finish(() => resolve(c));
        },
        (e) => {
          // Let go by us (ours was chosen, or the page gave up); else
          // failed, which matters only if Passwords has nothing to offer.
          if (theirs.signal.aborted) return;
          if (!ours) finish(() => reject(e));
        },
      );
    });
  }

  for (const [name, fn] of [["create", create], ["get", get]]) {
    Object.defineProperty(container, name, {
      configurable: true,
      writable: true,
      value: {
        [name](options) {
          return fn(options);
        },
      }[name],
    });
  }

  // Passwords is a passkey authenticator on this device that can
  // suggest passkeys in a field.
  const yes = () => Promise.resolve(true);
  Object.defineProperty(PKC, "isUserVerifyingPlatformAuthenticatorAvailable", { configurable: true, writable: true, value: yes });
  Object.defineProperty(PKC, "isConditionalMediationAvailable", { configurable: true, writable: true, value: yes });
  const capabilities = PKC.getClientCapabilities;
  if (capabilities) {
    Object.defineProperty(PKC, "getClientCapabilities", {
      configurable: true,
      writable: true,
      value: async () => ({
        ...(await capabilities.call(PKC).catch(() => ({}))),
        conditionalGet: true,
        passkeyPlatformAuthenticator: true,
        userVerifyingPlatformAuthenticator: true,
      }),
    });
  }
})();
