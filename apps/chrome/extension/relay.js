// In every frame, from its start: carries passkeys.js's requests (in the
// page's own world) to the extension and its answers back, and tells the
// extension when the frame goes, so Passwords takes down what it was
// asking the person for it.

(() => {
  if (window.__oloRelay) return;
  window.__oloRelay = true;

  function tell(m) {
    try {
      return chrome.runtime.sendMessage(m).catch(() => null);
    } catch {
      return Promise.resolve(null);
    }
  }

  const answer = (rid, a) => window.postMessage({ __oloPasskey: "answer", rid, ...a }, "*");

  addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== window || m?.__oloPasskey !== true) return;
    if (m.kind === "request") {
      // (No extension to answer, as after an update: Chrome's own way.)
      tell({ type: "passkey", rid: m.rid, request: m.request }).then((a) => answer(m.rid, a || { native: true }));
    } else if (m.kind === "cancel") {
      tell({ type: "passkey-cancel", rid: m.rid });
    }
  });

  // A passkey chosen in the context menu for a sign-in waiting here.
  chrome.runtime.onMessage.addListener((m) => {
    if (m.type === "passkey-chosen") answer(m.rid, m.answer);
  });

  addEventListener("pagehide", () => tell({ type: "passkey-gone" }));
})();
