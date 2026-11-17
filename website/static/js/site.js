// olo-desktop.com: the copy buttons, and a screenshot enlarged on a
// click. (No framework, nothing that runs when nobody does anything.)
for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const text = document.getElementById(button.dataset.copy).innerText.replace(/^\$ /gm, "");
    try {
      await navigator.clipboard.writeText(text.trim());
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select and copy";
    }
    setTimeout(() => (button.textContent = "Copy"), 1800);
  });
}

// A screenshot, on a click (or Return on it): as large as the window has
// room for, over the page, in a <dialog> (so Escape closes it, the page
// behind is out of reach, and the keyboard goes back to the picture it
// came from). Its light or dark version, as the page shows it, at the
// width that size needs. A click anywhere, or Close, puts it away.
// Without this script the link opens the picture itself.
if (typeof HTMLDialogElement === "function") {
  let dialog = null;
  const close = () => dialog?.close();
  const show = (link) => {
    if (!dialog) {
      dialog = document.createElement("dialog");
      dialog.className = "zoomed";
      dialog.setAttribute("aria-label", "Screenshot, enlarged");
      dialog.addEventListener("click", close);
      dialog.addEventListener("close", () => dialog.replaceChildren());
      document.body.append(dialog);
    }
    const picture = link.querySelector("picture").cloneNode(true);
    // (As wide as the window, not the column it was in.)
    for (const source of picture.querySelectorAll("source, img")) source.sizes = "100vw";
    const img = picture.querySelector("img");
    img.loading = "eager";
    img.removeAttribute("fetchpriority");
    // Its size from its shape (the width and height it's written with),
    // not from whichever copy shows first: the page's smaller one is
    // shown while the larger loads, and the picture grew when it came
    // (Ben: "it first zooms a little then a second later zooms even
    // more").
    const w = Number(img.getAttribute("width")), h = Number(img.getAttribute("height"));
    if (w > 0 && h > 0) img.style.setProperty("--ratio", String(w / h));
    const button = document.createElement("button");
    button.type = "button";
    button.className = "zoomed-close";
    button.textContent = "Close";
    dialog.replaceChildren(picture, button);
    dialog.showModal();
  };
  for (const link of document.querySelectorAll("a.zoom")) {
    link.addEventListener("click", (e) => {
      // (A click meant for a new tab or a download is the link's own.)
      if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
      e.preventDefault();
      show(link);
    });
  }
}
