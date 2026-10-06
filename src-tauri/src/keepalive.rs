//! Background keepalive pings for active connection pools.
//!
//! An idle server-backed connection (Postgres, MySQL) can be silently
//! dropped by a NAT gateway, cloud load balancer, or corporate firewall
//! well before the user notices — the pool object survives in memory, but
//! the next query fails with an opaque driver error. A periodic
//! lightweight ping keeps the underlying socket (and, for tunnelled
//! connections, the SSH channel) exercised often enough to avoid most
//! idle-timeout drops, and doubles as the detector for the ones it can't
//! prevent: a failed ping is reported to the frontend via
//! [`CONNECTION_LOST_EVENT`] so the connection list can offer a one-click
//! reconnect instead of the user discovering it mid-query.
//!
//! Scope: only top-level profile connections (`connect` / `disconnect` in
//! `commands::connection`) get a heartbeat — the synthetic per-database
//! pools opened by `open_database_view` share the same underlying
//! TCP/tunnel liveness as their parent and are cheap to reopen on demand,
//! so a second heartbeat per open database would be redundant background
//! load for no real benefit.
//!
//! Lifecycle mirrors [`crate::db::ssh::SshTunnelHandle`]: the returned
//! [`KeepaliveHandle`] owns a [`CancellationToken`], cancelled on `Drop`.
//! Stashing it in [`crate::state::ActivePool`] alongside the SSH handle
//! means the loop stops automatically whenever that pool is removed or
//! replaced — no separate bookkeeping needed in `AppState`.

use crate::log_bus::{self, LogEntry, LogKind};
use crate::state::DbPool;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

/// Default ping interval. Short enough to stay well under common
/// NAT/load-balancer idle timeouts (typically 5+ minutes), long enough that
/// the ping traffic is negligible.
///
/// User-overridable via `connections.keepaliveSecs` (`0` disables the
/// heartbeat). Note the deliberate relationship with
/// [`crate::db::pool::IDLE_TIMEOUT`]: pinging more often than the pool reaps
/// means one connection per active pool never goes idle long enough to be
/// closed. That is the point — the next click is instant and a dead connection
/// is found before a query hits it — but it is a real, permanent socket per
/// open connection, which is why it can be turned off.
pub const DEFAULT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(180);

/// Tauri event name the frontend subscribes to.
pub const CONNECTION_LOST_EVENT: &str = "huginndb://connection-lost";

/// Emitted when a connection reported lost answers again.
pub const CONNECTION_RESTORED_EVENT: &str = "huginndb://connection-restored";

/// Payload for [`CONNECTION_LOST_EVENT`].
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionLostPayload {
    pub connection_id: String,
    pub error: String,
}

/// Payload for [`CONNECTION_RESTORED_EVENT`].
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionRestoredPayload {
    pub connection_id: String,
}

/// Waits before re-pinging a connection whose ping just failed, before it is
/// reported lost.
///
/// A single failed ping used to be the verdict. Most failures worth having a
/// heartbeat for are brief, though — a VPN reconnecting, a laptop waking, an
/// SSH tunnel redialling a session a firewall dropped — and reporting each one
/// put a card with a Reconnect button in front of the user for a connection
/// that was back before they could click it. Twenty seconds of patience in
/// total, after which the connection really is unreachable.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(15)];

/// How often a connection already reported lost is checked again.
///
/// The loop used to end at the first failure, so recovery was entirely
/// manual. It now keeps watching, more often than the healthy interval because
/// someone is waiting on the answer, and reports the connection restored the
/// moment it answers.
const LOST_RECHECK: Duration = Duration::from_secs(30);

/// Owns the background keepalive task for one connection pool. Dropping it
/// (pool removed on `disconnect`, or replaced by a fresh `connect` /
/// reconnect) cancels the loop.
pub struct KeepaliveHandle {
    cancel: CancellationToken,
}

impl Drop for KeepaliveHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Spawn a background task that pings `pool` every `interval`.
///
/// A failed ping is retried ([`RETRY_AFTER`]) before the connection is
/// reported lost via [`CONNECTION_LOST_EVENT`]. The loop then keeps checking
/// every [`LOST_RECHECK`] and reports [`CONNECTION_RESTORED_EVENT`] when the
/// connection answers again, which it usually does without anyone touching
/// it: the pool replaces sockets that fail their check
/// (`db::pool::check_before_use`) and an SSH tunnel redials a dead session
/// (`db::ssh::TunnelSession`). A manual reconnect still opens a fresh pool and
/// a fresh heartbeat, as before.
///
/// `last_used` is the [`crate::state::ActivePool`]'s usage stamp; a tick whose
/// interval the user's own queries already spanned is skipped, since traffic
/// that just succeeded is better proof of liveness than a `SELECT 1` and there
/// is no reason to pay for both.
///
/// `ping_timeout` is the connection's own introspection ceiling, handed in
/// already resolved: this function has a pool and an id, not an `AppState`, and
/// the caller is holding the profile at the moment it opens the connection.
/// Passing it matters because the failure is *reported*, not just logged — a
/// heartbeat that gives up in 20 s against a server the user has told the app
/// to wait 90 s for would flag a healthy connection as lost.
///
/// Returns `None` when `interval` is zero — the user has turned the heartbeat
/// off — so no task is spawned at all.
pub fn spawn(
    app: AppHandle,
    connection_id: String,
    pool: DbPool,
    interval: Duration,
    ping_timeout: Duration,
    last_used: Arc<AtomicU64>,
) -> Option<KeepaliveHandle> {
    if interval.is_zero() {
        return None;
    }
    let cancel = CancellationToken::new();
    let cancel_loop = cancel.clone();
    tokio::spawn(async move {
        let ping = || {
            crate::error::with_timeout_secs(
                ping_timeout,
                "keepalive ping",
                crate::db::exec::ping(&pool),
            )
        };
        let mut lost = false;
        loop {
            let wait = if lost { LOST_RECHECK } else { interval };
            if !sleep_or_cancel(&cancel_loop, wait).await {
                return;
            }
            if !lost {
                let idle =
                    crate::state::now_millis().saturating_sub(last_used.load(Ordering::Relaxed));
                if idle < interval.as_millis() as u64 {
                    continue;
                }
            }

            let mut result = ping().await;
            // Retry only on the way *into* "lost". Once reported, each recheck
            // is itself the retry.
            if !lost {
                for delay in RETRY_AFTER {
                    if result.is_ok() {
                        break;
                    }
                    if !sleep_or_cancel(&cancel_loop, delay).await {
                        return;
                    }
                    result = ping().await;
                }
            }

            match (result, lost) {
                (Ok(()), false) => {}
                (Ok(()), true) => {
                    lost = false;
                    log_bus::broadcast(
                        &app,
                        LogEntry::new(LogKind::Connection)
                            .connection_id(connection_id.clone())
                            .message("keepalive: connection answering again"),
                    );
                    let _ = app.emit(
                        CONNECTION_RESTORED_EVENT,
                        ConnectionRestoredPayload {
                            connection_id: connection_id.clone(),
                        },
                    );
                }
                (Err(_), true) => {}
                (Err(e), false) => {
                    lost = true;
                    let msg = e.to_string();
                    log_bus::broadcast(
                        &app,
                        LogEntry::new(LogKind::Connection)
                            .connection_id(connection_id.clone())
                            .message(
                                "keepalive: ping failed three times, flagging connection as lost",
                            )
                            .error(&msg),
                    );
                    let _ = app.emit(
                        CONNECTION_LOST_EVENT,
                        ConnectionLostPayload {
                            connection_id: connection_id.clone(),
                            error: msg,
                        },
                    );
                }
            }
        }
    });
    Some(KeepaliveHandle { cancel })
}

/// Sleep for `wait`, or return `false` as soon as the heartbeat is cancelled.
async fn sleep_or_cancel(cancel: &CancellationToken, wait: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(wait) => true,
    }
}
