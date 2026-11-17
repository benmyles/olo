// olo-desktop.com: the copy buttons, and nothing else. (No framework,
// nothing that runs when nobody does anything.)
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
