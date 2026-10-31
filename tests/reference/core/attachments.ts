import { relativeVaultPath } from "../markdown/markdown";
export function attachmentKind(value: string): "image" | "pdf" | undefined {
  let decoded = value;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    /* Keep malformed URLs as ordinary links. */
  }
  if (
    /\.pdf(?:[?#]|$)/i.test(decoded) ||
    /^data:application\/pdf[;,]/i.test(decoded)
  )
    return "pdf";
  if (
    /\.(png|jpe?g|gif|webp|svg|avif)(?:[?#]|$)/i.test(decoded) ||
    /^data:image\//i.test(decoded)
  )
    return "image";
}
export function attachmentURL(value: string, fromPath?: string) {
  if (/^https?:\/\//i.test(value)) return value;
  if (
    /^data:(?:image\/(?:png|jpeg|gif|webp|avif|svg\+xml)|application\/pdf)[;,]/i.test(
      value,
    )
  )
    return value;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith("//")) return;
  const hash = value.indexOf("#"),
    file = hash < 0 ? value : value.slice(0, hash),
    fragment = hash < 0 ? "" : value.slice(hash);
  const relative = relativeVaultPath(fromPath, file.split("?")[0]);
  return relative
    ? `quire://asset/${encodeURIComponent(relative)}${fragment}`
    : undefined;
}
