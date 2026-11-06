// Stands in for the Tauri opener plugin, which opens a link in the browser of the desktop.

/** Opens a web address in a new tab. */
export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}
