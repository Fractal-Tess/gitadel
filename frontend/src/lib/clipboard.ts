export async function copyText(value: string) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {
      // Clipboard access can still be denied in an otherwise supported browser.
    }
  }

  const carrier = document.createElement("textarea");
  carrier.value = value;
  carrier.setAttribute("readonly", "");
  carrier.style.position = "fixed";
  carrier.style.opacity = "0";
  document.body.append(carrier);
  carrier.select();

  try {
    if (!document.execCommand("copy")) throw new Error("Copy was rejected.");
  } finally {
    carrier.remove();
  }
}
