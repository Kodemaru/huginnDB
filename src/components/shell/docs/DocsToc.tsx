/**
 * "On this page": the `###`s of the section being read, with the one in view
 * marked, in a column beside the page.
 *
 * It is a second way to reach what the rail's nested rows already reach — the
 * rail lists a section's `###`s too — and the two never show together: the rail
 * hides them from `xl` up, exactly where there is width for this column. Below
 * that the column is hidden and the rail carries them, so a narrow window loses
 * nothing.
 */

import { useTranslation } from "react-i18next";
import type { DocSubsection } from "@/lib/appInfo/docOutline";
import { TreeRow } from "@/components/ui/tree-row";
import { CONTROL_FOCUS_TIGHT, MICRO_HEADING } from "@/components/ui/styles";
import { cn } from "@/lib/utils";

interface Props {
  subs: DocSubsection[];
  /** The `###` slug in view, from `useScrollSpy`. */
  active: string | null;
  onPick: (slug: string) => void;
}

export function DocsToc({ subs, active, onPick }: Props) {
  const { t } = useTranslation();
  if (subs.length === 0) return null;
  return (
    <nav
      aria-label={t("docs.onThisPage")}
      className="hidden w-52 shrink-0 overflow-y-auto border-l border-border/60 px-3 py-5 xl:block"
    >
      <div className={cn(MICRO_HEADING, "mb-2 px-2")}>{t("docs.onThisPage")}</div>
      {subs.map((s) => (
        <TreeRow
          key={s.slug}
          onClick={() => onPick(s.slug)}
          aria-current={s.slug === active ? "location" : undefined}
          className={cn(
            "rounded-md px-2 py-1 text-left text-xs leading-snug transition-colors",
            CONTROL_FOCUS_TIGHT,
            s.slug === active
              ? "bg-brand/10 text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {s.title}
        </TreeRow>
      ))}
    </nav>
  );
}
