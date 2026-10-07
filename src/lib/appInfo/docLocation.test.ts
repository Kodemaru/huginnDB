import { describe, expect, it } from "vitest";
import en from "@/lib/i18n/locales/en.json";
import es from "@/lib/i18n/locales/es.json";
import { docLocation } from "./docs";

/**
 * Links from the UI into the documentation carry a heading through i18n and
 * resolve it against the doc in the viewer's language (see `docLocation`).
 * A heading renamed in one place and not the other would quietly open the
 * doc's cover instead, so every such link is pinned here, per language.
 */
describe("docLocation", () => {
  const links: { doc: string; heading: (locale: typeof en) => string }[] = [
    {
      // Settings → Connections, the "?" beside the per-server counts.
      doc: "connections",
      heading: (l) => l.settings.connections.live.docHeading,
    },
  ];

  for (const [lang, locale] of [
    ["en", en],
    ["es", es],
  ] as const) {
    it(`finds every linked heading in the ${lang} docs`, () => {
      for (const link of links) {
        const where = docLocation(link.doc, link.heading(locale), lang);
        expect(where, `${link.doc} → "${link.heading(locale)}"`).not.toBeNull();
        // A `###` inside a section: the viewer opens the section and scrolls.
        expect(where?.section).toBeTruthy();
        expect(where?.anchor).toBeTruthy();
      }
    });
  }

  it("answers null for a heading that is not there", () => {
    expect(docLocation("connections", "No such heading", "en")).toBeNull();
    expect(docLocation("no-such-doc", "Anything", "en")).toBeNull();
  });
});
