/**
 * Full-text search over the bundled documentation, for the viewer's own search
 * box.
 *
 * The rule is the one `commandPalette/searchSettings` already teaches the user
 * — every word typed must appear, accents and case are ignored — applied to
 * *pages* rather than to settings. A page is what the viewer shows: a doc's
 * cover, or one of its `##` sections. A hit therefore always names somewhere the
 * viewer can open, and the `###` it sits under when there is one, so the click
 * scrolls to the passage instead of dropping the reader at the top of a page
 * and leaving them to hunt for it.
 *
 * It searches the language the viewer would show (`getDocBody`), so a Spanish
 * UI finds Spanish words in Spanish pages. The index is built per language, once:
 * ten docs are a few thousand lines, which is small enough that nothing here
 * needs to be incremental, and large enough that rebuilding it per keystroke
 * would be felt.
 *
 * Fenced code is searched — someone looking for `--server-name` should find the
 * flag table's neighbour, and the config snippets are where those strings live —
 * but a `###` inside a fence is not a heading, the same rule `parseDoc` follows,
 * so the index of a sub-heading here lines up with `ParsedDoc.sections[].subs`.
 */

import { DOCS, getDocBody } from "./docs";
import { parseDoc, plainText } from "./docOutline";

const FENCE = /^\s*```/;
const HEADING = /^(#{1,6})\s+(.*)$/;

/**
 * Fold a string to lower case without accents, **keeping its length**.
 *
 * `searchSettings.foldForSearch` is not enough here: it folds the whole string,
 * and a character that changes length when folded (`İ`, a decomposed accent that
 * is not a combining mark) shifts every offset after it, which would put the
 * highlight a letter to the left of the word it is meant to mark. Folding per
 * character and falling back to the character itself when the result is not one
 * unit keeps the folded and original strings index-for-index.
 */
export function foldAligned(text: string): string {
  let out = "";
  for (const ch of text) {
    const f = ch
      .normalize("NFD")
      .replace(/\p{Diacritic}/gu, "")
      .toLowerCase();
    out += f.length === ch.length ? f : ch;
  }
  return out;
}

/** A line of context with the spans to emphasise, as `[start, end)` offsets. */
export interface DocSnippet {
  text: string;
  ranges: [number, number][];
}

export interface DocHit {
  docId: string;
  /** `##` slug of the page, or `null` for the doc's cover. */
  section: string | null;
  /** `###` slug to scroll to inside that page. */
  anchor: string | null;
  /** The page's title (the `##`), `null` on a cover. */
  sectionTitle: string | null;
  /** The `###` the passage sits under, when it does. */
  subTitle: string | null;
  /** The passage, `null` when the hit is a title rather than a body match. */
  snippet: DocSnippet | null;
  /** Lower is better: 0 a page title, 1 a sub-heading, 2 body text. */
  rank: 0 | 1 | 2;
}

interface IndexedLine {
  text: string;
  folded: string;
  /** Position among the page's `###`s, or `-1` before the first. */
  sub: number;
  heading: boolean;
}

interface IndexedPage {
  docId: string;
  section: string | null;
  title: string | null;
  foldedTitle: string;
  subs: { slug: string; title: string; folded: string }[];
  lines: IndexedLine[];
  /** The whole page folded, for the "does every word appear" test. */
  folded: string;
}

const indexes = new Map<string, IndexedPage[]>();

function indexPage(
  docId: string,
  section: string | null,
  title: string | null,
  body: string,
  subs: { slug: string; title: string }[],
): IndexedPage {
  const lines: IndexedLine[] = [];
  let inFence = false;
  let sub = -1;
  for (const raw of body.split("\n")) {
    let heading = false;
    if (FENCE.test(raw)) {
      inFence = !inFence;
    } else if (!inFence) {
      const m = HEADING.exec(raw);
      if (m) {
        heading = true;
        if (m[1].length === 3) sub += 1;
      }
    }
    const text = plainText(raw)
      .replace(/^\s*(?:#{1,6}\s+|[-*+]\s+|\d+\.\s+|>\s?)/, "")
      .replace(/\s*\|\s*/g, " · ")
      .replace(/^[\s·]+|[\s·]+$/g, "")
      .trim();
    if (!text || /^[-:· ]+$/.test(text)) continue;
    lines.push({ text, folded: foldAligned(text), sub, heading });
  }
  return {
    docId,
    section,
    title,
    foldedTitle: foldAligned(title ?? ""),
    subs: subs.map((s) => ({ ...s, folded: foldAligned(s.title) })),
    lines,
    folded: lines.map((l) => l.folded).join("\n"),
  };
}

function indexFor(lang: string): IndexedPage[] {
  const hit = indexes.get(lang);
  if (hit) return hit;
  const pages: IndexedPage[] = [];
  for (const doc of DOCS) {
    const parsed = parseDoc(getDocBody(doc, lang));
    pages.push(indexPage(doc.id, null, null, parsed.cover, []));
    for (const s of parsed.sections) {
      pages.push(indexPage(doc.id, s.slug, s.title, s.body, s.subs));
    }
  }
  indexes.set(lang, pages);
  return pages;
}

/** Whole-word-ish positions of every query word in `folded`, merged. */
function rangesOf(folded: string, words: string[]): [number, number][] {
  const found: [number, number][] = [];
  for (const w of words) {
    let from = 0;
    for (;;) {
      const at = folded.indexOf(w, from);
      if (at === -1) break;
      found.push([at, at + w.length]);
      from = at + w.length;
    }
  }
  found.sort((a, b) => a[0] - b[0]);
  const merged: [number, number][] = [];
  for (const r of found) {
    const last = merged[merged.length - 1];
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
    else merged.push([r[0], r[1]]);
  }
  return merged;
}

const SNIPPET_WIDTH = 150;

/** The passage to show for a line, trimmed to a window around the first match. */
function snippetOf(line: IndexedLine, words: string[]): DocSnippet {
  const all = rangesOf(line.folded, words);
  if (line.text.length <= SNIPPET_WIDTH) return { text: line.text, ranges: all };
  const first = all[0]?.[0] ?? 0;
  const start = Math.max(0, Math.min(first - 40, line.text.length - SNIPPET_WIDTH));
  const end = Math.min(line.text.length, start + SNIPPET_WIDTH);
  const lead = start > 0 ? "…" : "";
  const tail = end < line.text.length ? "…" : "";
  const text = lead + line.text.slice(start, end) + tail;
  const shift = lead.length - start;
  const ranges = all
    .filter(([a, b]) => b > start && a < end)
    .map(
      ([a, b]) =>
        [Math.max(a, start) + shift, Math.min(b, end) + shift] as [number, number],
    );
  return { text, ranges };
}

/** Most results the viewer will list; the rest is a refinement away. */
export const MAX_DOC_HITS = 60;

/** The query is ignored below this length: one letter matches every page. */
export const MIN_QUERY_LENGTH = 2;

/**
 * Pages of the bundled docs matching every word of `query`, best first.
 *
 * Per page it can yield several hits — the page's own title, each `###` whose
 * title matches, and the best body line — because those are different places to
 * land, not three spellings of one. Within a rank the order is the registry's,
 * then the page's, so results read in the order the guides do.
 */
export function searchDocs(query: string, lang: string): DocHit[] {
  const words = foldAligned(query).split(/\s+/).filter(Boolean);
  if (words.length === 0 || query.trim().length < MIN_QUERY_LENGTH) return [];

  const hits: (DocHit & { order: number })[] = [];
  let order = 0;
  for (const page of indexFor(lang)) {
    order += 1;
    const base = {
      docId: page.docId,
      section: page.section,
      sectionTitle: page.title,
      order,
    };

    if (page.title && words.every((w) => page.foldedTitle.includes(w))) {
      hits.push({ ...base, anchor: null, subTitle: null, snippet: null, rank: 0 });
    }
    page.subs.forEach((sub) => {
      if (words.every((w) => sub.folded.includes(w))) {
        hits.push({
          ...base,
          anchor: sub.slug,
          subTitle: sub.title,
          snippet: null,
          rank: 1,
        });
      }
    });

    if (!words.every((w) => page.folded.includes(w))) continue;
    // The line that holds the most distinct words; first wins a tie, so the
    // earliest mention is what the reader is shown.
    let best: IndexedLine | null = null;
    let bestCount = 0;
    for (const line of page.lines) {
      if (line.heading) continue;
      const count = words.filter((w) => line.folded.includes(w)).length;
      if (count > bestCount) {
        best = line;
        bestCount = count;
      }
    }
    // Every word appears on the page but never in one body line together with
    // another — fall back to the first line carrying any of them, so the hit
    // still has a passage to show.
    if (!best) best = page.lines.find((l) => !l.heading) ?? null;
    if (!best) continue;
    const sub = page.subs[best.sub];
    hits.push({
      ...base,
      anchor: sub?.slug ?? null,
      subTitle: sub?.title ?? null,
      snippet: snippetOf(best, words),
      rank: 2,
    });
  }

  hits.sort((a, b) => a.rank - b.rank || a.order - b.order);
  return hits.slice(0, MAX_DOC_HITS).map(({ order: _order, ...hit }) => hit);
}

/** Spans of `text` matching `query`, for emphasising a title in a result row. */
export function highlightRanges(text: string, query: string): [number, number][] {
  const words = foldAligned(query).split(/\s+/).filter(Boolean);
  return rangesOf(foldAligned(text), words);
}
