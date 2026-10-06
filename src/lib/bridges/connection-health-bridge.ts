/**
 * Wires the Rust `huginndb://connection-lost` and
 * `huginndb://connection-restored` Tauri events (see
 * `src-tauri/src/keepalive.rs`) into `stores/connectionHealth.ts`.
 *
 * Mount once at App startup — re-subscribing every render would attach
 * duplicate listeners (HMR / StrictMode).
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useConnectionHealth } from "@/stores/session/connectionHealth";
import { useConnections } from "@/stores/session/connections";
import {
  connectAndWarm,
  disconnectAndClean,
} from "@/lib/connection/connectFlow";
import i18n from "@/lib/i18n";
import { notify } from "@/lib/notify";

const CONNECTION_LOST_EVENT = "huginndb://connection-lost";
const CONNECTION_RESTORED_EVENT = "huginndb://connection-restored";

interface ConnectionLostPayload {
  connection_id: string;
  error: string;
}

interface ConnectionRestoredPayload {
  connection_id: string;
}

const lostGroup = (id: string) => `connection-lost:${id}`;

function profileName(id: string): string {
  return useConnections.getState().profiles.find((p) => p.id === id)?.name ?? id;
}

export async function startConnectionHealthBridge(): Promise<UnlistenFn> {
  const unlistenLost = await listen<ConnectionLostPayload>(
    CONNECTION_LOST_EVENT,
    (event) => {
      const { connection_id: id, error } = event.payload;
      const already = id in useConnectionHealth.getState().lost;
      useConnectionHealth.getState().markLost(id, error);
      // The badge the tree and the status bar grow is the *state*; this is the
      // *event*, and it is the one notification in the app nobody asked for —
      // the heartbeat fires whether or not the connections panel is even on
      // screen, and the first thing anyone learns otherwise is a cryptic driver
      // error mid-query. Carrying "Reconnect" escalates it to a card, which is
      // right: a dead pool is not a thing to glance at.
      //
      // Only on the transition into "lost": a card per report for a server
      // that stays down is worse than none.
      if (already) return;
      notify.warning(i18n.t("connections.lostTitle", { name: profileName(id) }), {
        description: error,
        actions: [
          {
            label: i18n.t("connections.reconnect"),
            variant: "primary",
            onClick: () => {
              void disconnectAndClean(id).then(() => connectAndWarm(id));
            },
          },
        ],
        // One card per connection, however many times it is re-reported.
        group: lostGroup(id),
      });
    },
  );
  // The heartbeat keeps checking a lost connection, and most of them come back
  // on their own: the pool replaces dead sockets and an SSH tunnel redials its
  // session. When one does, the lost badge and the card offering a manual
  // Reconnect are both untrue, so both go.
  const unlistenRestored = await listen<ConnectionRestoredPayload>(
    CONNECTION_RESTORED_EVENT,
    (event) => {
      const { connection_id: id } = event.payload;
      if (!(id in useConnectionHealth.getState().lost)) return;
      useConnectionHealth.getState().clear(id);
      notify.dismissGroup(lostGroup(id));
      notify.success(
        i18n.t("connections.restoredTitle", { name: profileName(id) }),
      );
    },
  );
  return () => {
    unlistenLost();
    unlistenRestored();
  };
}
