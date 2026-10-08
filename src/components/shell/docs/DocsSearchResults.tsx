/**
 * What the viewer's search box shows in place of the page: every passage of the
 * guides that matches, grouped by guide, each one a row that opens the page it is
 * on and scrolls to the heading it sits under.
 *
 * It lists and navigates; it never renders the markdown. The reader lands on the
 * real page, where the surrounding text is, rather than on a second, search-only
 * copy of it — the same split `SettingsSearchResults` makes for settings.
 */

import * as React from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";
import { getDoc } from "@/lib/appInfo/docs";
import {
  MIN_QUERY_LENGTH,
  highlightRanges,
  type DocHit,
  type DocSnippet,
} from "@/lib/appInfo/docSearch";
import { TreeRow } from "@/components/ui/tree-row";
import { CONTROL_FOCUS_TIGHT } from "@/components/ui/styles";
import { cn } from "@/lib/utils";
import { docIcon } from "./docIcons";

/** `text` with the given `[start, end)` spans wrapped in `<mark>`. */
function Marked({ text, ranges }: { text: string; ranges: [number, number][] }) {
  const out: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach(([a, b], i) => {
    if (a > at) out.push(text.slice(at, a));
    out.push(
      <mark key={i} className="rounded-sm bg-brand/20 px-0.5 text-foreground">
        {text.slice(a, b)}
      </mark>,
    );
    at = b;
  });
  if (at < text.length) out.push(text.slice(at));
  return <>{out}</>;
}

function Snippet({ snippet }: { snippet: DocSnippet }) {
  return (
    <span className="line-clamp-2 text-xs leading-snug text-muted-foreground">
      <Marked text={snippet.text} ranges={snippet.ranges} />
    </span>
  );
}

interface Props {
  query: string;
  hits: DocHit[];
  onPick: (hit: DocHit) => void;
}

export function DocsSearchResults({ query, hits, onPick }: Props) {
  const { t } = useTranslation();

  if (query.trim().length < MIN_QUERY_LENGTH) {
    return (
      <p className="py-6 text-sm text-muted-foreground">
        {t("docs.search.tooShort", { min: MIN_QUERY_LENGTH })}
      </p>
    );
  }
  if (hits.length === 0) {
    return (
      <div className="py-6">
        <p className="text-sm text-foreground">
          {t("docs.search.none", { query: query.trim() })}
        </p>
        <p className="mt-1 text-xs text-muted-foreground">{t("docs.search.noneHint")}</p>
      </div>
    );
  }

  // Hits arrive ranked, so a guide's heading is drawn where its first hit is and
  // its later hits are folded under that heading even if another guide's better
  // hit came between them.
  const order: string[] = [];
  const byDoc = new Map<string, DocHit[]>();
  for (const hit of hits) {
    if (!byDoc.has(hit.docId)) {
      byDoc.set(hit.docId, []);
      order.push(hit.docId);
    }
    byDoc.get(hit.docId)!.push(hit);
  }

  return (
    <div className="space-y-5">
      {order.map((id) => {
        const doc = getDoc(id);
        if (!doc) return null;
        const Icon = docIcon(id);
        return (
          <section key={id} aria-label={t(doc.titleKey)}>
            <h3 className="mb-1.5 flex items-center gap-2 text-xs font-medium text-foreground">
              <Icon aria-hidden className="h-3.5 w-3.5 text-brand" />
              {t(doc.titleKey)}
            </h3>
            <div className="overflow-hidden rounded-lg border border-border">
              {byDoc.get(id)!.map((hit, i) => {
                const page = hit.sectionTitle ?? t("docs.cover");
                const title = hit.subTitle ? `${page} › ${hit.subTitle}` : page;
                return (
                  <TreeRow
                    key={`${hit.section}:${hit.anchor}:${hit.rank}:${i}`}
                    onClick={() => onPick(hit)}
                    className={cn(
                      "items-start gap-3 border-b border-border/60 px-4 py-2.5 text-left transition-colors last:border-b-0",
                      CONTROL_FOCUS_TIGHT,
                      "focus-visible:ring-inset",
                    )}
                  >
                    <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span className="text-sm text-foreground">
                        <Marked text={title} ranges={highlightRanges(title, query)} />
                      </span>
                      {hit.snippet && <Snippet snippet={hit.snippet} />}
                    </span>
                    <ChevronRight
                      aria-hidden
                      className="mt-1 h-3.5 w-3.5 shrink-0 text-muted-foreground"
                    />
                  </TreeRow>
                );
              })}
            </div>
          </section>
        );
      })}
    </div>
  );
}
