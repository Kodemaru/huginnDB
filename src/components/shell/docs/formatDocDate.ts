/** A doc's last-updated date as the reader's language writes it, or `null`. */
export function formatDocDate(iso: string | null, lang: string): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  return d.toLocaleDateString(lang, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}
