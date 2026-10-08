/**
 * The viewer's left rail: a search box, the home page, then the guides under
 * their group headings — the same anatomy as Preferences' rail, so the two
 * dialogs read as one family.
 *
 * The open guide unfolds into its pages (the cover and one row per `##`), and
 * the open page's `###`s hang under it, but only below `xl`: from there up the
 * "On this page" column shows them (`DocsToc`), and listing them twice would be
 * the one place the two disagree when a heading is renamed.
 */

import * as React from "react";
import { useTranslation } from "react-i18next";
import { Home } from "lucide-react";
import { DOC_GROUPS, docsInGroup } from "@/lib/appInfo/docs";
import type { ParsedDoc } from "@/lib/appInfo/docOutline";
import { NavRailItem } from "@/components/ui/nav-rail";
import { SearchField } from "@/components/ui/search-field";
import { TreeRow } from "@/components/ui/tree-row";
import { CONTROL_FOCUS_TIGHT, MICRO_HEADING } from "@/components/ui/styles";
import { cn } from "@/lib/utils";
import { docIcon } from "./docIcons";

interface Props {
  query: string;
  onQuery: (query: string) => void;
  onSearchEnter: () => void;
  searchRef: React.Ref<HTMLInputElement>;
  /** `null` is the home page. */
  activeId: string | null;
  /** The active guide's outline; `null` on the home page. */
  parsed: ParsedDoc | null;
  sectionSlug: string | null;
  /** While a search is showing, nothing in the rail is "current". */
  searching: boolean;
  onHome: () => void;
  onDoc: (id: string) => void;
  onSection: (slug: string | null, anchor?: string) => void;
}

function PageRow({
  label,
  active,
  depth = 1,
  className,
  onClick,
}: {
  label: string;
  active: boolean;
  depth?: 1 | 2;
  className?: string;
  onClick: () => void;
}) {
  return (
    <TreeRow
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={cn(
        "rounded-md py-1 pr-2 text-left leading-snug transition-colors",
        CONTROL_FOCUS_TIGHT,
        depth === 1 ? "pl-2 text-xs" : "pl-5 text-2xs",
        active
          ? "bg-brand/10 text-foreground"
          : "text-muted-foreground hover:text-foreground",
        className,
      )}
    >
      {label}
    </TreeRow>
  );
}

export function DocsRail({
  query,
  onQuery,
  onSearchEnter,
  searchRef,
  activeId,
  parsed,
  sectionSlug,
  searching,
  onHome,
  onDoc,
  onSection,
}: Props) {
  const { t } = useTranslation();
  const section = parsed?.sections.find((s) => s.slug === sectionSlug) ?? null;

  return (
    <aside className="flex min-h-0 flex-col border-r border-border bg-card/40">
      <div className="shrink-0 p-2 pb-1">
        <SearchField
          ref={searchRef}
          value={query}
          onValueChange={onQuery}
          onClear={() => onQuery("")}
          clearLabel={t("docs.search.clear")}
          size="sm"
          placeholder={t("docs.search.placeholder")}
          aria-label={t("docs.search.label")}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSearchEnter();
          }}
        />
      </div>
      <nav
        aria-label={t("docs.title")}
        className="min-h-0 flex-1 space-y-3 overflow-y-auto p-2"
      >
        <NavRailItem
          icon={Home}
          label={t("docs.home")}
          active={!searching && activeId === null}
          onClick={onHome}
        />
        {DOC_GROUPS.map((group) => {
          const docs = docsInGroup(group);
          if (docs.length === 0) return null;
          return (
            <div key={group} className="space-y-0.5">
              <div className={cn(MICRO_HEADING, "px-2.5 pb-1")}>
                {t(`docs.groups.${group}`)}
              </div>
              {docs.map((doc) => {
                const open = doc.id === activeId;
                return (
                  <React.Fragment key={doc.id}>
                    <NavRailItem
                      icon={docIcon(doc.id)}
                      label={t(doc.titleKey)}
                      active={!searching && open && sectionSlug === null}
                      aria-expanded={open}
                      onClick={() => onDoc(doc.id)}
                    />
                    {open && parsed && (
                      <div className="ml-[18px] space-y-0.5 border-l border-border/70 pl-1.5">
                        {parsed.sections.map((s) => (
                          <React.Fragment key={s.slug}>
                            <PageRow
                              label={s.title}
                              active={!searching && s.slug === sectionSlug}
                              onClick={() => onSection(s.slug)}
                            />
                            {s.slug === section?.slug &&
                              s.subs.map((sub) => (
                                <PageRow
                                  key={sub.slug}
                                  label={sub.title}
                                  depth={2}
                                  active={false}
                                  className="xl:hidden"
                                  onClick={() => onSection(s.slug, sub.slug)}
                                />
                              ))}
                          </React.Fragment>
                        ))}
                      </div>
                    )}
                  </React.Fragment>
                );
              })}
            </div>
          );
        })}
      </nav>
    </aside>
  );
}
