import { openUrl } from "@tauri-apps/plugin-opener";

// The single seam between the webview and the operating system's browser: link
// rendering calls these two functions, so the markdown tests stub one module
// instead of faking a Tauri runtime. Only `http(s)` targets leave the app —
// a relative path or an anchor is a document-local reference that must never
// navigate the GUI away from the conversation.

export function isExternalUrl(href: string | undefined): href is string {
  return href !== undefined && /^https?:\/\//i.test(href);
}

export async function openExternal(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch (error) {
    // A failed launch must not break the transcript; the link just does
    // nothing and the console keeps the reason.
    console.error("failed to open external link", error);
  }
}
