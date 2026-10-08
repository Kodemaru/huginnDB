/**
 * The viewer's home page: every guide, grouped the way the rail groups them,
 * each with what it is for.
 *
 * It exists because opening Documentation used to land inside the first guide.
 * Someone who came for MCP saw Connections, and the one-line descriptions the
 * registry carries for exactly this purpose were only ever read by the command
 * palette. Here they are the point — a card per guide, so "which of these is the
 * one I want" is answered before anything is opened.
 */

import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";
import { DOC_GROUPS, docsInGroup, getDocBody } from "@/lib/appInfo/docs";
import { parseDoc } from "@/lib/appInfo/docOutline";
import { TreeRow } from "@/components/ui/tree-row";
import { CONTROL_FOCUS_TIGHT, MICRO_HEADING } from "@/components/ui/styles";
import { cn } from "@/lib/utils";
import { docIcon } from "./docIcons";
import { formatDocDate } from "./formatDocDate";

export function DocsHome({ onOpen }: { onOpen: (id: string) => void }) {
  const { t, i18n } = useTranslation();
  const lang = i18n.language;

  return (
    <div className="space-y-7">
      {DOC_GROUPS.map((group) => {
        const docs = docsInGroup(group);
        if (docs.length === 0) return null;
        return (
          <section key={group} aria-label={t(`docs.groups.${group}`)}>
            <h3 className={cn(MICRO_HEADING, "mb-2")}>{t(`docs.groups.${group}`)}</h3>
            <div className="grid gap-3 md:grid-cols-2">
              {docs.map((doc) => {
                const Icon = docIcon(doc.id);
                const sections = parseDoc(getDocBody(doc, lang)).sections.length;
                const date = formatDocDate(doc.updated, lang);
                return (
                  <TreeRow
                    key={doc.id}
                    onClick={() => onOpen(doc.id)}
                    className={cn(
                      "group items-start gap-3 rounded-lg border border-border bg-card/40 p-3.5 text-left transition-colors hover:border-brand/60 hover:bg-accent",
                      CONTROL_FOCUS_TIGHT,
                    )}
                  >
                    <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-brand/10 text-brand">
                      <Icon className="h-[18px] w-[18px]" />
                    </span>
                    <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span className="text-sm font-medium text-foreground">
                        {t(doc.titleKey)}
                      </span>
                      <span className="text-xs leading-snug text-muted-foreground">
                        {t(doc.descriptionKey)}
                      </span>
                      <span className="mt-1 text-3xs text-muted-foreground">
                        {t("docs.sectionCount", { count: sections })}
                        {date ? ` · ${t("docs.updated", { date })}` : ""}
                      </span>
                    </span>
                    <ChevronRight
                      aria-hidden
                      className="mt-1 h-4 w-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5"
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
