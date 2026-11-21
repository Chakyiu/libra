// Start screen logic. Runs on the bundled page only; after a server URL opens,
// this page is gone and the server serves its own UI.
const form = document.getElementById("server-form");
const input = document.getElementById("server-url");
const error = document.getElementById("error");

function showError(message) {
  error.textContent = message;
  error.hidden = !message;
}

async function prefillLastUrl() {
  try {
    const saved = await window.__TAURI__.core.invoke("last_server_url");
    if (saved && !input.value) input.value = saved;
  } catch {
    // First run, or the bundled page is not under Tauri; the field stays empty.
  }
  input.focus();
  input.select();
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  showError("");
  const button = form.querySelector("button");
  button.disabled = true;
  try {
    await window.__TAURI__.core.invoke("open_server_url", { url: input.value });
    // Navigation replaces this page; nothing to do on success.
  } catch (err) {
    showError(typeof err === "string" ? err : String(err));
    button.disabled = false;
    input.focus();
  }
});

document.addEventListener("DOMContentLoaded", prefillLastUrl);
