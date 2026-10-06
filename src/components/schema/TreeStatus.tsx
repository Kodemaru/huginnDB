import { useTranslation } from "react-i18next";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";

/**
 * Shimmer rows standing in for a part of the schema tree that is being read.
 *
 * Exists because the tree had three different ways of saying "loading" and the
 * one that mattered most said nothing: `refresh` creates a connection's slice
 * the moment it starts, and the explorers only showed "Loading schema…" while
 * there was *no* slice — so for the whole of a `list_tables` round trip an
 * expanded database rendered an empty subtree, indistinguishable from an empty
 * database or a failed one. The shimmer reads as an active fetch rather than a
 * stall; `label` stays the accessible status text.
 */
export function TreeSkeleton({
  label,
  rows = 3,
  className,
}: {
  label: string;
  rows?: number;
  className?: string;
}) {
  return (
    <div className={className ?? "space-y-1 py-1"} role="status" aria-label={label}>
      {Array.from({ length: rows }, (_, i) => (
        <div
          key={i}
          className="h-2.5 animate-pulse rounded-sm bg-muted-foreground/15"
          style={{ width: `${70 - (i % 4) * 12}%` }}
        />
      ))}
    </div>
  );
}

/**
 * A failed schema read, said where it happened, with the way out next to it.
 *
 * The slice is marked `initialized` even when its read fails — on purpose, so
 * the explorer's mount effect does not retry in a loop — which means nothing
 * ever retries on its own. Until this existed the only way back was finding
 * "Refresh" in a context menu; the button puts it on the line that explains
 * why it is needed.
 */
export function SchemaLoadError({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-start gap-2 px-3 py-2 text-xs text-destructive">
      <span className="min-w-0 flex-1 break-words">{message}</span>
      <Button
        variant="outline"
        size="xs"
        className="shrink-0 text-foreground"
        onClick={onRetry}
      >
        <RefreshCw className="h-3 w-3" />
        {t("common.retry")}
      </Button>
    </div>
  );
}
