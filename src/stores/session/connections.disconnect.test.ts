/**
 * The order a disconnect happens in.
 *
 * The backend takes a pool out of its map at once but then waits for it to
 * close gracefully — seconds behind an SSH tunnel. When the window was only
 * updated after that wait, the connection looked connected throughout: the
 * click felt slow, and any schema read in flight failed against the vanished
 * pool and was reported as "could not read the schema", because its slice
 * still existed. The window must let go first.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

let releaseBackend!: () => void;
const disconnect = vi.fn(
  () => new Promise<void>((resolve) => (releaseBackend = resolve)),
);
const drop = vi.fn();
const closeForConnection = vi.fn();

vi.mock("@/lib/tauri", () => ({ api: { disconnect } }));
vi.mock("@/lib/i18n", () => ({ default: { t: (k: string) => k } }));
vi.mock("@/lib/notify", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/stores/grid/filterHistory", () => ({
  useFilterHistory: { getState: () => ({ clearForConnection: vi.fn() }) },
}));
vi.mock("@/stores/grid/fkOptions", () => ({
  fkOptionsCache: { clearConnection: vi.fn() },
}));
vi.mock("@/stores/session/persistedTabs", () => ({
  flushTabState: vi.fn().mockResolvedValue(undefined),
  hydrateTabState: vi.fn().mockResolvedValue(undefined),
  persistLaunchState: vi.fn().mockResolvedValue(undefined),
  subscribedConnectionIds: () => [],
}));
vi.mock("@/stores/session/schema", () => ({
  useSchema: { getState: () => ({ drop }) },
}));
vi.mock("@/stores/session/tabs", () => ({
  useTabs: { getState: () => ({ tabs: [], closeForConnection }) },
}));
vi.mock("@/lib/dockview", () => ({ clearProtectedPanelsForConnection: vi.fn() }));
vi.mock("@/lib/connection/passwordPrompt", () => ({
  askForPassword: vi.fn(),
  rememberAskedPassword: vi.fn(),
}));

const { useConnections } = await import("@/stores/session/connections");

beforeEach(() => {
  vi.clearAllMocks();
  useConnections.setState({ active: new Set(["p"]) });
});

describe("disconnect", () => {
  it("lets the window go before the backend has finished closing the pool", async () => {
    const done = useConnections.getState().disconnect("p");
    // Let the flush and the window teardown run; the backend stays pending.
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledWith("p"));

    expect(useConnections.getState().active.has("p")).toBe(false);
    expect(drop).toHaveBeenCalledWith("p");

    releaseBackend();
    await done;
  });

  it("closes the tabs with the tree when asked, before the backend has finished", async () => {
    const done = useConnections.getState().disconnect("p", { closeTabs: true });
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledWith("p"));
    expect(closeForConnection).toHaveBeenCalledWith("p");
    releaseBackend();
    await done;
  });

  it("keeps the tabs when not asked to — a reconnect disconnects to keep them", async () => {
    const done = useConnections.getState().disconnect("p");
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledWith("p"));
    releaseBackend();
    await done;
    expect(closeForConnection).not.toHaveBeenCalledWith("p");
  });
});
