/**
 * The strip above every page of the viewer: a glyph, the title, one line saying
 * what the page is, and the actions that belong to it.
 *
 * The same shape as the header of a Preferences section — icon tile, `h2`,
 * muted line, actions on the right — so the two dialogs have the same top edge.
 */

import * as React from "react";

export function DocsPageHeader({
  icon: Icon,
  title,
  eyebrow,
  description,
  actions,
}: {
  icon: React.ComponentType<{ className?: string }>;
  title: React.ReactNode;
  /** A breadcrumb above the title, when the page is inside a guide. */
  eyebrow?: React.ReactNode;
  description?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  return (
    <div className="flex shrink-0 items-start gap-3 px-6 pb-4 pt-5">
      <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-brand/10 text-brand">
        <Icon className="h-[18px] w-[18px]" />
      </div>
      <div className="min-w-0 flex-1">
        {eyebrow && <div className="mb-0.5 text-2xs text-muted-foreground">{eyebrow}</div>}
        <h2 className="text-lg font-semibold leading-tight tracking-tight">{title}</h2>
        {description && (
          <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1.5 pt-0.5">{actions}</div>}
    </div>
  );
}
