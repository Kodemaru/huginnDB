import {
  Activity,
  BookOpen,
  Bot,
  Cable,
  FileJson,
  Keyboard,
  Layers,
  Leaf,
  Network,
  Server,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react";

/**
 * The glyph each guide wears in the rail, on the home page and in a page header.
 *
 * Kept beside the viewer rather than in the registry (`lib/appInfo/docs.ts`):
 * the registry is plain data a test and the command palette read, and a `lucide`
 * component in it would make every one of those import the icon set. A guide
 * with no entry here falls back to the book, so adding one to the registry
 * never breaks the viewer — it only leaves it plain until someone picks a glyph.
 *
 * Where a guide is also a Preferences section the glyph is that section's own
 * (`SettingsDialog`'s `NAV`), so the two dialogs agree on what "MCP" looks like.
 */
const ICONS: Record<string, LucideIcon> = {
  connections: Network,
  environments: Layers,
  shortcuts: Keyboard,
  jsonSchemas: FileJson,
  mongodb: Leaf,
  sqlserver: Server,
  pulse: Activity,
  mcp: Cable,
  ai: Bot,
  policy: ShieldCheck,
};

export function docIcon(id: string): LucideIcon {
  return ICONS[id] ?? BookOpen;
}
