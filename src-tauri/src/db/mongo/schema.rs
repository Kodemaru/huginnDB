//! MongoDB introspection: databases, collections, inferred fields, indexes.
//!
//! MongoDB is schemaless, so there is no catalog to read column types from the
//! way the SQL drivers do. Field lists are **inferred** by sampling documents
//! ([`infer_columns`]); the result is best-effort and reflects only what the
//! sample contained. Everything here returns the same DTOs the SQL explorer
//! uses ([`DatabaseInfo`], [`TableInfo`], [`ColumnInfo`], [`IndexInfo`]) so the
//! frontend tree renders MongoDB without a separate code path.

use crate::commands::schema::{
    ColumnInfo, DatabaseInfo, DatabaseSize, IndexInfo, PrivilegeInfo, TableInfo, UserInfo,
};
use crate::db::ddl::{ColumnDef, IndexDef, TableStructure};
use crate::error::{AppError, AppResult};
use crate::state::MongoConn;
use futures_util::stream::{self, StreamExt};
use mongodb::bson::{doc, Bson, Document};
use mongodb::results::CollectionType;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Number of documents sampled when inferring a collection's field list.
const SAMPLE_SIZE: i64 = 100;

/// How long `$sample` gets before field inference gives up on it.
///
/// Short on purpose, and it is a **discriminator rather than a patience
/// setting**. `$sample` has two execution strategies and nothing in between:
/// under the conditions we meet (first stage, N far below 5% of the
/// collection, more than 100 documents) the server serves it from a
/// pseudo-random cursor and answers in milliseconds; when any precondition
/// does not hold it silently falls back to reading the whole collection and
/// sorting it by a random key. On tens of millions of documents that second
/// path does not finish — it either blows the 100 MB in-memory sort limit or
/// runs for minutes.
///
/// So there is no value of this constant that lets a slow-path `$sample`
/// succeed; a longer one only makes the user wait longer for the same
/// fallback. Two seconds is well past any round trip the fast path needs,
/// including a remote server, and short enough that the fallback is the cost
/// of a pause rather than of a hang. It was 8s when this was first bounded,
/// chosen to match the pool's `server_selection_timeout` — which was the wrong
/// thing to match, because that governs *reaching* a server and this governs
/// *which algorithm the server picked*.
///
/// The same value bounds the fallback `find`, where it means the ordinary
/// thing: that read is O(N) in the page size and cannot be slow for being big,
/// so a stall there really is an unresponsive server.
const INFER_TIMEOUT_MS: u64 = 2_000;

/// Resolve the [`mongodb::Database`] a connection handle targets, or fail if no
/// database has been selected (the parent cluster connection before the user
/// expands a database in the explorer).
pub fn resolve_db(conn: &MongoConn) -> AppResult<mongodb::Database> {
    let name = conn
        .database
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(
                "no database selected — expand a database in the explorer or include one in the \
             connection URI (mongodb://host/<database>)"
                    .into(),
            )
        })?;
    Ok(conn.client.database(name))
}

/// List every database in the cluster.
pub async fn list_databases(conn: &MongoConn) -> AppResult<Vec<DatabaseInfo>> {
    let names = conn.client.list_database_names().await?;
    Ok(names
        .into_iter()
        .map(|name| DatabaseInfo { name })
        .collect())
}

/// On-disk size per database, from `listDatabases`' `sizeOnDisk`.
///
/// **The fallback is not defensive padding — it is the difference between
/// losing the sizes and losing the tree.** `DatabaseSpecification` declares
/// `size_on_disk: u64` and `empty: bool` with neither `Option` nor
/// `serde(default)`, so a server or a role that omits either field fails the
/// *whole* deserialize, and `list_databases()` returns an error rather than a
/// partial list. Since this is a best-effort badge, that error must degrade to
/// "no sizes" and never to "no databases": the name-only call is the same one
/// [`list_databases`] already makes, so whatever the tree could show before it
/// can still show.
///
/// `sizeOnDisk` is WiredTiger's **compressed** on-disk figure, which is why it
/// is typically far below the sum of this database's collection sizes and does
/// not compare with any other driver's number.
pub async fn database_sizes(conn: &MongoConn) -> AppResult<Vec<DatabaseSize>> {
    match conn.client.list_databases().await {
        Ok(specs) => Ok(specs
            .into_iter()
            .map(|d| DatabaseSize {
                name: d.name,
                size_bytes: Some(d.size_on_disk),
            })
            .collect()),
        Err(_) => Ok(conn
            .client
            .list_database_names()
            .await?
            .into_iter()
            .map(|name| DatabaseSize {
                name,
                size_bytes: None,
            })
            .collect()),
    }
}

/// One `listCollections` row: the collection's name and whether it is a view.
struct CollectionSpec {
    name: String,
    is_view: bool,
}

/// Read the target database's catalog: one round trip, no statistics.
///
/// `None` for a parent cluster connection with no database selected — the
/// explorer browses collections only after the user expands a specific
/// database (a synthetic `<id>::db::<name>` child pool is opened then). The
/// callers answer that with an empty list rather than erroring via
/// `resolve_db`, mirroring MySQL's `list_tables` returning `Ok(vec![])` when
/// `SELECT DATABASE()` is NULL. Without this the frontend's parallel
/// `listDatabases()` + `listTables()` boot probe rejects and blanks the entire
/// tree for a multi-DB Mongo connection (#52).
async fn read_catalog(conn: &MongoConn) -> AppResult<Option<(String, Vec<CollectionSpec>)>> {
    if no_database_selected(conn) {
        return Ok(None);
    }
    let db = resolve_db(conn)?;
    let mut cursor = db.list_collections().await?;
    let mut specs = Vec::new();
    while cursor.advance().await? {
        let spec = cursor.deserialize_current()?;
        specs.push(CollectionSpec {
            is_view: spec.collection_type == CollectionType::View,
            name: spec.name,
        });
    }
    Ok(Some((db.name().to_string(), specs)))
}

fn table_info(db_name: &str, spec: CollectionSpec, stats: CollectionStats) -> TableInfo {
    TableInfo {
        schema: db_name.to_string(),
        name: spec.name,
        kind: if spec.is_view {
            "view".into()
        } else {
            "table".into()
        },
        row_count: stats.count,
        size_bytes: stats.size_bytes,
    }
}

/// List the collections (and views) of the target database **without** their
/// statistics — a single catalog round trip.
///
/// This is what the schema tree draws from. Document counts and sizes cost a
/// round trip *per collection*, and behind an SSH tunnel that was the whole of
/// the wait: a database with ninety-odd collections took two and a half seconds
/// to appear, nearly all of it spent on the badges. The tree now shows the
/// names at once and asks for the figures separately
/// ([`list_collections_with_stats`] through `get_table_stats`), the same split
/// `get_database_sizes` already makes for databases.
pub async fn list_collections(conn: &MongoConn) -> AppResult<Vec<TableInfo>> {
    let Some((db_name, specs)) = read_catalog(conn).await? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<TableInfo> = specs
        .into_iter()
        .map(|spec| table_info(&db_name, spec, CollectionStats::default()))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// [`list_collections`] plus each collection's approximate document count and
/// on-disk size.
///
/// For every caller that wants the figures and is not a person waiting on a
/// tree: the schema tree's deferred badge pass, the MCP `list_tables` tool,
/// the dump's progress estimate.
///
/// One stats read per collection, several in flight at once. This used to be
/// an `estimated_document_count` per collection awaited inside the catalog
/// loop — strictly sequential. Concurrency halves the wait behind a tunnel
/// rather than dividing it by [`COLL_STATS_CONCURRENCY`] (measured: 2.6 s to
/// 1.3–1.8 s for 96 collections), which is why the tree no longer waits for
/// this at all. `buffered` keeps the output in catalog order.
pub async fn list_collections_with_stats(conn: &MongoConn) -> AppResult<Vec<TableInfo>> {
    let Some((db_name, specs)) = read_catalog(conn).await? else {
        return Ok(Vec::new());
    };
    let db = resolve_db(conn)?;
    // Owned values into each future rather than borrows: a stream of futures
    // borrowing from this frame trips the higher-ranked lifetime check that a
    // `Send` Tauri command future imposes. `Database` is an `Arc` handle, so
    // the clones are cheap.
    let stats_denied = Arc::new(AtomicBool::new(false));
    let jobs: Vec<(String, bool)> = specs.iter().map(|s| (s.name.clone(), s.is_view)).collect();
    let stats: Vec<CollectionStats> = stream::iter(jobs)
        .map(|(name, is_view)| {
            let db = db.clone();
            let stats_denied = Arc::clone(&stats_denied);
            async move {
                // A view has no storage of its own to report, and counting one
                // would run its whole pipeline.
                if is_view {
                    CollectionStats::default()
                } else {
                    collection_stats(&db, &name, &stats_denied).await
                }
            }
        })
        .buffered(COLL_STATS_CONCURRENCY)
        .collect()
        .await;

    let mut out: Vec<TableInfo> = specs
        .into_iter()
        .zip(stats)
        .map(|(spec, stats)| table_info(&db_name, spec, stats))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Reject a collection name no user operation may target, and return the
/// trimmed name callers should actually use.
///
/// Lives here rather than in [`super::indexes`] (its first caller) because
/// every collection-level write needs the same checks and a second copy would
/// be one edit away from disagreeing with this one. `pub` rather than
/// `pub(super)` since 1.23.1: `commands::schema` creates collections too —
/// directly (`create_collection`) and as the second half of creating a MongoDB
/// database — and had grown its own inline copy of the first two checks.
pub fn validate_collection(collection: &str) -> AppResult<&str> {
    let name = collection.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("no collection given".into()));
    }
    if name.starts_with("system.") {
        return Err(AppError::InvalidInput(
            "`system.` is reserved for MongoDB's own collections".into(),
        ));
    }
    // `$` is legal only inside MongoDB's internal namespaces, which the check
    // above already refuses, and a NUL truncates the name on the wire rather
    // than being rejected — so a name containing either would create something
    // other than what the user asked for.
    if name.contains('$') || name.contains('\0') {
        return Err(AppError::InvalidInput(
            "a collection name cannot contain '$' or a NUL byte".into(),
        ));
    }
    Ok(name)
}

/// Rename a collection, optionally moving it into another database.
///
/// MongoDB's `renameCollection` is a command on the **`admin`** database and
/// takes fully-qualified `db.collection` names on both sides, which is also
/// what makes moving between databases free: the destination is just a
/// different qualifier. That is why this doesn't go through
/// [`resolve_db`]-scoped helpers the way the rest of this module does — but the
/// *source* database still comes from the connection handle, so a rename can
/// only ever start from the database the user is browsing.
///
/// Two deliberate refusals:
///
/// * **`dropTarget` is always `false`.** Renaming onto an existing collection
///   must surface as an error, never as a silent drop of whatever was there.
/// * **A view cannot be renamed.** MongoDB has no rename for one; the only
///   path is drop + recreate, which is a destructive gesture rather than a
///   rename (the same reasoning `docs/MONGODB_ROADMAP.md` records for the view
///   editor). The check is made here rather than left to the server so the
///   message says what to do instead.
///
/// Note that a cross-database rename copies the documents server-side, so it
/// is proportional to the collection's size and needs a role with rights on
/// both databases — the caller warns about both before submitting.
pub async fn rename_collection(
    conn: &MongoConn,
    from: &str,
    to: &str,
    to_db: Option<&str>,
) -> AppResult<()> {
    let from = validate_collection(from)?;
    let to = validate_collection(to)?;
    let db = resolve_db(conn)?;
    let src_db = db.name().to_string();
    let dst_db = match to_db.map(str::trim).filter(|s| !s.is_empty()) {
        Some(name) => name.to_string(),
        None => src_db.clone(),
    };
    if src_db == dst_db && from == to {
        return Ok(());
    }
    reject_view_rename(&db, from).await?;
    conn.client
        .database("admin")
        .run_command(doc! {
            "renameCollection": format!("{src_db}.{from}"),
            "to": format!("{dst_db}.{to}"),
            "dropTarget": false,
        })
        .await?;
    Ok(())
}

/// Fail if `name` is a view rather than a collection. See
/// [`rename_collection`] for why this is refused up front.
async fn reject_view_rename(db: &mongodb::Database, name: &str) -> AppResult<()> {
    let mut cursor = db.list_collections().filter(doc! {"name": name}).await?;
    while cursor.advance().await? {
        if cursor.deserialize_current()?.collection_type == CollectionType::View {
            return Err(AppError::InvalidInput(format!(
                "`{name}` is a view — MongoDB cannot rename a view; recreate it under the new \
                 name and drop this one"
            )));
        }
    }
    Ok(())
}

/// How many per-collection stats reads [`list_collections_with_stats`] keeps
/// in flight.
///
/// Below the five-connection pool a person's connection is granted, so a
/// database listing never queues behind itself and leaves a socket free for
/// the grid the user may be waiting on at the same moment. The MCP's
/// two-connection pool simply queues the excess inside the driver, which is
/// still far better than the strictly sequential loop this replaced.
const COLL_STATS_CONCURRENCY: usize = 4;

/// What [`list_collections_with_stats`] reports per collection besides its name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CollectionStats {
    count: Option<u64>,
    size_bytes: Option<u64>,
}

/// Document count and on-disk size (data + indexes) of one collection, in one
/// round trip.
///
/// `$collStats` with `storageStats` answers both from WiredTiger's metadata —
/// no scan — which is the same cost `estimated_document_count` already had, so
/// the size comes for free.
///
/// This replaced a single `$collStats` run through `Database::aggregate`, which
/// **never worked**: `$collStats` is a collection-level stage that a
/// database-level aggregation does not accept, and since the call was
/// best-effort its error was swallowed and no MongoDB collection ever showed a
/// size. The parse had a second, independent bug that would have hidden the
/// sizes even had the stage run: it read them with `get_i64`, and the server
/// answers small collections' figures as `Int32`. See [`stats_from_docs`].
///
/// Falls back to `estimated_document_count` — the behaviour before this
/// existed, minus the size — when the stats read fails, which is what a role
/// without the `collStats` privilege gets. The first such failure sets
/// `denied`, so a listing under that role pays for the refused stage a handful
/// of times (however many were already in flight) rather than once per
/// collection.
async fn collection_stats(
    db: &mongodb::Database,
    name: &str,
    denied: &AtomicBool,
) -> CollectionStats {
    let coll = db.collection::<Document>(name);
    if !denied.load(Ordering::Relaxed) {
        let pipeline = vec![
            doc! {"$collStats": {"storageStats": {}}},
            // `storageStats` carries WiredTiger's whole per-collection report
            // (tens of KB on a collection with many indexes); keep only what
            // the tree shows rather than shipping that through a tunnel.
            doc! {"$project": {
                "storageStats.count": 1,
                "storageStats.totalSize": 1,
                "storageStats.storageSize": 1,
                "storageStats.size": 1,
            }},
        ];
        match coll.aggregate(pipeline).await {
            Ok(mut cursor) => {
                let mut docs = Vec::new();
                while let Ok(true) = cursor.advance().await {
                    if let Ok(d) = cursor.deserialize_current() {
                        docs.push(d);
                    }
                }
                let stats = stats_from_docs(&docs);
                if stats.count.is_some() {
                    return stats;
                }
                // Answered, but without a count (an unusual storage engine):
                // keep the size and still try the estimate below.
                return CollectionStats {
                    count: coll.estimated_document_count().await.ok(),
                    ..stats
                };
            }
            // Only a refusal stops the others from trying. Any other failure is
            // about *this* collection (a system namespace, an exotic type) and
            // must not cost every other collection in the listing its size.
            Err(e) if is_unauthorized(&e) => denied.store(true, Ordering::Relaxed),
            Err(_) => {}
        }
    }
    CollectionStats {
        count: coll.estimated_document_count().await.ok(),
        size_bytes: None,
    }
}

/// Fold the documents a `$collStats` stage returned into one collection's
/// figures.
///
/// Several documents, not one, because a **sharded** collection answers with
/// one per shard, and its count and size are the sums. A shard that omits a
/// figure contributes nothing; a figure no document carries stays `None`
/// rather than becoming a misleading zero.
///
/// Every width the server might use is accepted. `Int32` is the case that
/// matters: it is what a small collection's figures arrive as, and reading
/// them with `get_i64` is why sizes never appeared.
fn stats_from_docs(docs: &[Document]) -> CollectionStats {
    let mut count: Option<u64> = None;
    let mut size: Option<u64> = None;
    for d in docs {
        let Ok(storage) = d.get_document("storageStats") else {
            continue;
        };
        if let Some(n) = storage.get("count").and_then(bson_u64) {
            count = Some(count.unwrap_or(0) + n);
        }
        let shard_size = ["totalSize", "storageSize", "size"]
            .iter()
            .find_map(|k| storage.get(*k).and_then(bson_u64));
        if let Some(n) = shard_size {
            size = Some(size.unwrap_or(0) + n);
        }
    }
    CollectionStats {
        count,
        size_bytes: size,
    }
}

/// Whether the server refused a command for lack of privilege (`Unauthorized`,
/// code 13) — the answer every collection in the database will get, as
/// opposed to a failure particular to one of them.
fn is_unauthorized(e: &mongodb::error::Error) -> bool {
    matches!(e.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13)
}

/// A non-negative BSON number as `u64`, whatever width the server chose.
fn bson_u64(value: &Bson) -> Option<u64> {
    match value {
        Bson::Int32(v) => Some((*v).max(0) as u64),
        Bson::Int64(v) => Some((*v).max(0) as u64),
        Bson::Double(v) if v.is_finite() => Some(v.max(0.0) as u64),
        _ => None,
    }
}

/// What the catalog says a name is, for the two introspection decisions that
/// depend on it: whether `$sample` can take its fast path, and whether the
/// relation can be asked for indexes at all.
///
/// One enum rather than two booleans because both answers come out of the same
/// `listCollections` spec, and a caller that needs both should pay for one
/// round trip, not two. [`table_structure`] is that caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RelationKind {
    /// An ordinary collection: sampleable, and has its own indexes.
    Collection,
    /// A user-defined view — a stored aggregation pipeline, per
    /// [`super::aggregation`].
    View,
    /// A time-series collection, which is itself a view over its
    /// `system.buckets.*` backing collection.
    Timeseries,
}

impl RelationKind {
    /// Whether asking `$sample` for a field sample can possibly be cheap.
    ///
    /// False for both view kinds, for one shared reason: an aggregation against
    /// a view is rewritten to run the view's own pipeline first, so our
    /// `$sample` is no longer the first stage and never gets the random cursor
    /// that makes it fast. It degrades to reading everything the view produces
    /// and sorting it by a random key — on a view over millions of documents
    /// that does not finish, it either exceeds the 100 MB in-memory sort limit
    /// or runs for minutes.
    ///
    /// This is not a heuristic about size, it is a fact about the relation:
    /// asking `$sample` there is pointless, and the only thing waiting for it
    /// buys is the wait.
    fn sample_can_be_fast(self) -> bool {
        matches!(self, Self::Collection)
    }

    /// Whether `listIndexes` may be sent for this relation.
    ///
    /// A view has no indexes of its own — the server rejects the command
    /// outright with `CommandNotSupportedOnView` (code 166) rather than
    /// answering empty — so asking is not a cheap "probably nothing", it is a
    /// guaranteed error. A time-series collection does accept `listIndexes`,
    /// which is why this is narrower than [`Self::sample_can_be_fast`].
    fn has_own_indexes(self) -> bool {
        !matches!(self, Self::View)
    }
}

/// Classify one `listCollections` spec.
///
/// Follows the same rule `spec_is_view` documents: a spec with no `type` is a
/// collection (the field only appeared in MongoDB 3.4), and an unrecognised
/// reply falls to the ordinary answer rather than the exotic one — here that
/// means the sample is attempted and indexes are asked for, which is the
/// behaviour of every server old enough not to report a type.
pub(super) fn relation_kind_from_spec(spec: &Document) -> RelationKind {
    match spec.get_str("type") {
        Ok("view") => RelationKind::View,
        Ok("timeseries") => RelationKind::Timeseries,
        _ => RelationKind::Collection,
    }
}

/// Ask the catalog what `name` is. One filtered `listCollections` — a catalog
/// command, no scan — answered before any documents are read.
///
/// The cost is that every inference pays that round trip, including the small
/// collections where `$sample` would have answered in milliseconds. A catalog
/// lookup is single-digit milliseconds against a nearby server and the
/// alternative is a two-second stall on exactly the relations big enough for
/// someone to need the field list, so the trade is worth making. An unreadable
/// catalog answers [`RelationKind::Collection`], which is the behaviour this
/// replaced.
pub(super) async fn relation_kind(conn: &MongoConn, name: &str) -> RelationKind {
    match super::aggregation::collection_spec(conn, name).await {
        Ok(Some(spec)) => relation_kind_from_spec(&spec),
        _ => RelationKind::Collection,
    }
}

/// Read up to `SAMPLE_SIZE` documents without `$sample`: the oldest half in
/// natural order and the newest half in reverse.
///
/// Both ends, not just the head, because the head alone is the wrong sample for
/// the shape this exists to serve. An append-ordered collection — measurements,
/// events, logs — has its *oldest* documents first, so a plain
/// `find().limit(100)` describes the schema as it was when the collection was
/// young and misses every field added since. Reading the tail as well costs one
/// more round trip and catches schema drift in the direction it actually
/// travels.
///
/// The reverse pass is best-effort: `$natural` sorting is not accepted
/// everywhere (a time-series collection is a view over its buckets, and views
/// restrict what they will sort by), and half a sample is a fine outcome when
/// the alternative is none.
async fn scan_both_ends(coll: &mongodb::Collection<Document>) -> AppResult<Vec<Document>> {
    let half = (SAMPLE_SIZE / 2).max(1);
    let mut docs = Vec::new();

    let mut head = coll
        .find(doc! {})
        .limit(half)
        .max_time(Duration::from_millis(INFER_TIMEOUT_MS))
        .await?;
    while head.advance().await? {
        docs.push(head.deserialize_current()?);
    }

    let tail = coll
        .find(doc! {})
        .sort(doc! {"$natural": -1})
        .limit(half)
        .max_time(Duration::from_millis(INFER_TIMEOUT_MS))
        .await;
    if let Ok(mut tail) = tail {
        while let Ok(true) = tail.advance().await {
            if let Ok(d) = tail.deserialize_current() {
                docs.push(d);
            }
        }
    }
    Ok(docs)
}

/// Infer a collection's field list by sampling documents.
///
/// Returns one [`ColumnInfo`] per distinct top-level field seen across the
/// sample, `_id` first. `data_type` is the BSON type name of the field's first
/// observed value; `nullable` is true when the field was absent from at least
/// one sampled document (so it is not guaranteed present). `is_primary_key` is
/// set only for `_id`.
///
/// Two strategies, chosen from the catalog rather than by trying one and
/// waiting (see [`RelationKind::sample_can_be_fast`]). `$sample` is preferred
/// wherever it can be cheap — a random sample describes a heterogeneous
/// collection better than any fixed window — and [`scan_both_ends`] covers the
/// rest, including the case where `$sample` was tried and failed for a reason
/// the catalog could not predict.
pub async fn infer_columns(conn: &MongoConn, collection: &str) -> AppResult<Vec<ColumnInfo>> {
    let kind = relation_kind(conn, collection).await;
    infer_columns_of(conn, collection, kind).await
}

/// [`infer_columns`] for a caller that has already classified the relation, so
/// the catalog is read once per describe rather than once per half of it.
pub(super) async fn infer_columns_of(
    conn: &MongoConn,
    collection: &str,
    kind: RelationKind,
) -> AppResult<Vec<ColumnInfo>> {
    let db = resolve_db(conn)?;
    let coll = db.collection::<Document>(collection);

    let mut documents: Option<Vec<Document>> = None;

    if kind.sample_can_be_fast() {
        let sampled = coll
            .aggregate(vec![doc! {"$sample": {"size": SAMPLE_SIZE}}])
            .max_time(Duration::from_millis(INFER_TIMEOUT_MS))
            .await;
        if let Ok(mut cursor) = sampled {
            let mut docs = Vec::new();
            let mut ok = true;
            loop {
                match cursor.advance().await {
                    Ok(true) => match cursor.deserialize_current() {
                        Ok(d) => docs.push(d),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    },
                    Ok(false) => break,
                    // A `$sample` that fails partway through (the slow path
                    // hitting its sort limit reports on the first batch, not at
                    // dispatch) is a failed sample, not a partial one.
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                documents = Some(docs);
            }
        }
    }

    let documents = match documents {
        Some(d) => d,
        None => scan_both_ends(&coll).await?,
    };

    // Preserve first-seen field order; track type + how many docs contained it.
    let mut order: Vec<String> = Vec::new();
    let mut types: std::collections::HashMap<String, &'static str> =
        std::collections::HashMap::new();
    let mut present: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let sampled = documents.len();

    for docu in &documents {
        for (k, v) in docu {
            if !types.contains_key(k) {
                order.push(k.clone());
                types.insert(k.clone(), super::values::bson_type_name(v));
            }
            *present.entry(k.clone()).or_insert(0) += 1;
        }
    }

    // An empty collection samples zero documents, so `order` would come back
    // empty too — there is no catalog to fall back to the way `fetch_table_data`
    // does for the SQL drivers (#27), because MongoDB has no schema outside the
    // documents themselves. Every document gets an `_id` regardless, so seed it
    // here: without it `cols` is empty, the grid has no PK to build a WHERE
    // clause with, and the insert affordance hides along with it, leaving an
    // empty collection with no way to write its first document from the UI.
    if order.is_empty() {
        order.push("_id".to_string());
    }

    // Ensure `_id` is first even if the sample happened to order it later.
    order.sort_by_key(|k| if k == "_id" { 0 } else { 1 });

    let cols = order
        .into_iter()
        .map(|name| {
            let always_present = present.get(&name).copied().unwrap_or(0) >= sampled.max(1);
            ColumnInfo {
                data_type: types.get(&name).copied().unwrap_or("null").to_string(),
                nullable: name != "_id" && !always_present,
                is_primary_key: name == "_id",
                referenced_schema: None,
                referenced_table: None,
                referenced_column: None,
                name,
            }
        })
        .collect();
    Ok(cols)
}

/// List the indexes defined on a collection.
///
/// A view is answered with an empty list rather than an error: it has no
/// indexes of its own, and `listIndexes` against one fails with
/// `CommandNotSupportedOnView` (code 166). "This relation has no indexes" is
/// the true answer, and letting the driver's error out instead turns a
/// perfectly describable view into a failed panel — which is exactly how this
/// was found (see [`table_structure`]).
pub async fn list_indexes(conn: &MongoConn, collection: &str) -> AppResult<Vec<IndexInfo>> {
    if !relation_kind(conn, collection).await.has_own_indexes() {
        return Ok(Vec::new());
    }
    list_indexes_of_collection(conn, collection).await
}

/// [`list_indexes`] once the relation is known to accept `listIndexes`, so
/// [`table_structure`] does not pay a second catalog round trip to learn what
/// it already knows.
async fn list_indexes_of_collection(
    conn: &MongoConn,
    collection: &str,
) -> AppResult<Vec<IndexInfo>> {
    let db = resolve_db(conn)?;
    let coll = db.collection::<Document>(collection);
    let mut cursor = coll.list_indexes().await?;
    let mut out = Vec::new();
    while cursor.advance().await? {
        let model = cursor.deserialize_current()?;
        let columns: Vec<String> = model.keys.keys().cloned().collect();
        let name = model
            .options
            .as_ref()
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| columns.join("_"));
        let unique = model
            .options
            .as_ref()
            .and_then(|o| o.unique)
            .unwrap_or(false);
        out.push(IndexInfo {
            name,
            columns,
            unique,
        });
    }
    Ok(out)
}

/// Build a read-only [`TableStructure`] for a collection (inferred fields +
/// real indexes). MongoDB has no foreign keys; structure *editing* is deferred
/// to the roadmap, so the visual editor renders this in read-only mode.
///
/// **Why a view is described leniently.** This is the first half of
/// `commands::structure::describe_relation_inner`, whose second half reads the
/// view's stored pipeline — the thing a caller asking about a view actually
/// wants. Both halves used to be strict, so on a view the first one decided
/// whether the second ever ran: `listIndexes` failed with code 166 on every
/// view, and on a heavy one the field sample timed out before that, and either
/// way `describe_table` returned an error instead of the pipeline it had not
/// tried to read yet. A view whose fields cannot be sampled inside
/// [`INFER_TIMEOUT_MS`] is therefore described with an empty column list rather
/// than not described at all: fields are inferred here and inference is
/// best-effort by construction (see the module doc), whereas the pipeline is
/// the view's actual definition and is read from the catalog.
///
/// Sampling a *collection* stays strict. A failure there is a real one — an
/// unreachable server, a lost privilege — and silently reporting a collection
/// as having no fields would be the structure editor's problem, not a
/// describe's.
pub async fn table_structure(conn: &MongoConn, collection: &str) -> AppResult<TableStructure> {
    let db_name = resolve_db(conn)?.name().to_string();
    let kind = relation_kind(conn, collection).await;

    let columns = match infer_columns_of(conn, collection, kind).await {
        Ok(columns) => columns,
        Err(_) if kind == RelationKind::View => Vec::new(),
        Err(e) => return Err(e),
    };
    let indexes = if kind.has_own_indexes() {
        list_indexes_of_collection(conn, collection).await?
    } else {
        Vec::new()
    };

    let column_defs = columns
        .into_iter()
        .map(|c| ColumnDef {
            name: c.name,
            original_name: None,
            data_type: c.data_type,
            nullable: c.nullable,
            default: None,
            is_primary_key: c.is_primary_key,
            auto_increment: false,
        })
        .collect();

    let index_defs = indexes
        .into_iter()
        .map(|i| IndexDef {
            name: Some(i.name),
            columns: i.columns,
            unique: i.unique,
        })
        .collect();

    Ok(TableStructure {
        schema: Some(db_name),
        name: collection.to_string(),
        columns: column_defs,
        indexes: index_defs,
        foreign_keys: vec![],
    })
}

/// Roles that imply admin-equivalent access, used to derive
/// [`UserInfo::is_superuser`]. Not exhaustive (custom roles can grant
/// equivalent power) — best-effort, matching the built-in role names
/// MongoDB ships.
const SUPERUSER_ROLES: &[&str] = &[
    "root",
    "dbOwner",
    "userAdminAnyDatabase",
    "dbAdminAnyDatabase",
    "clusterAdmin",
    "atlasAdmin",
];

/// Whether the connection has no database selected (the parent cluster
/// connection before the user expands a specific database in the explorer).
fn no_database_selected(conn: &MongoConn) -> bool {
    conn.database.as_deref().filter(|s| !s.is_empty()).is_none()
}

/// List the users defined on the resolved database via `usersInfo`.
///
/// MongoDB scopes users per-database (not per-cluster), so this normally
/// mirrors [`list_collections`] in only covering the database the connection
/// handle currently targets. A parent cluster connection has no database to
/// scope to, though — unlike `list_collections` (which can just return an
/// empty list, since collections are inherently per-database), "who can log
/// into this cluster" has a real cluster-wide answer: `usersInfo` accepts
/// `{forAllDBs: true}` to return every user on every database, run against
/// any database (`admin` here, mirroring [`ping`]'s cluster-level probe).
pub async fn list_users(conn: &MongoConn) -> AppResult<Vec<UserInfo>> {
    let result = if no_database_selected(conn) {
        conn.client
            .database("admin")
            .run_command(doc! {"usersInfo": 1, "forAllDBs": true})
            .await?
    } else {
        resolve_db(conn)?.run_command(doc! {"usersInfo": 1}).await?
    };
    let users = result.get_array("users").cloned().unwrap_or_default();

    let mut out: Vec<UserInfo> = users
        .into_iter()
        .filter_map(|u| u.as_document().cloned())
        .map(|u| {
            let name = u.get_str("user").unwrap_or("").to_string();
            let roles: Vec<(String, bool)> = u
                .get_array("roles")
                .ok()
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_document())
                .map(|r| {
                    let role = r.get_str("role").unwrap_or("").to_string();
                    let db_name = r.get_str("db").unwrap_or("").to_string();
                    let is_super = SUPERUSER_ROLES.contains(&role.as_str());
                    (format!("{role}@{db_name}"), is_super)
                })
                .collect();
            UserInfo {
                name,
                is_superuser: roles.iter().any(|(_, is_super)| *is_super),
                // MongoDB has no per-account lock flag reachable via
                // usersInfo; a user document existing means it can log in.
                can_login: true,
                roles: roles.into_iter().map(|(r, _)| r).collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// List `user`'s effective privileges (resource + action) on the resolved
/// database, via `usersInfo` with `showPrivileges: true`.
///
/// Same cluster-wide fallback as [`list_users`]: without a selected database
/// there is no single `db` to qualify `user` with, so the lookup runs
/// `{forAllDBs: true}` against `admin` and filters the returned users by name
/// client-side instead (a user with the same name can exist on more than one
/// database; privileges from every match are concatenated).
pub async fn list_privileges(conn: &MongoConn, user: &str) -> AppResult<Vec<PrivilegeInfo>> {
    let users = if no_database_selected(conn) {
        let result = conn
            .client
            .database("admin")
            .run_command(doc! {
                "usersInfo": {"forAllDBs": true},
                "showPrivileges": true,
            })
            .await?;
        result
            .get_array("users")
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|u| {
                u.as_document()
                    .and_then(|d| d.get_str("user").ok())
                    .is_some_and(|name| name == user)
            })
            .collect()
    } else {
        let db = resolve_db(conn)?;
        let db_name = db.name().to_string();
        let result = db
            .run_command(doc! {
                "usersInfo": {"user": user, "db": db_name},
                "showPrivileges": true,
            })
            .await?;
        result.get_array("users").cloned().unwrap_or_default()
    };

    let mut out = Vec::new();
    for u in users.iter().filter_map(|u| u.as_document()) {
        let privs = u
            .get_array("inheritedPrivileges")
            .ok()
            .into_iter()
            .flatten();
        for p in privs.filter_map(|p| p.as_document()) {
            let resource = p.get_document("resource").ok();
            let schema = resource
                .and_then(|r| r.get_str("db").ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let table = resource
                .and_then(|r| r.get_str("collection").ok())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            for action in p.get_array("actions").ok().into_iter().flatten() {
                if let Some(action) = action.as_str() {
                    out.push(PrivilegeInfo {
                        privilege: action.to_string(),
                        schema: schema.clone(),
                        table: table.clone(),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Engine and version, as `mongodb <version>`.
///
/// `buildInfo` is an `admin`-database run-command, so this works regardless of
/// which database the connection is bound to. Reports `?` rather than failing if
/// the reply lacks a `version` field — a status-bar string is not worth turning
/// into an error.
pub async fn server_version(conn: &MongoConn) -> AppResult<String> {
    let info = conn
        .client
        .database("admin")
        .run_command(mongodb::bson::doc! {"buildInfo": 1})
        .await?;
    let ver = info.get_str("version").unwrap_or("?");
    Ok(format!("mongodb {ver}"))
}

/// Best-effort liveness check used by `test_connection`: ping the admin db.
pub async fn ping(conn: &MongoConn) -> AppResult<()> {
    conn.client
        .database("admin")
        .run_command(doc! {"ping": 1})
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Folding `$collStats` output into the tree's count and size
    // -----------------------------------------------------------------------

    /// The shape a real server returned for a 185 600-document collection:
    /// every figure an `Int32`. Reading these with `get_i64` is why MongoDB
    /// collections never showed a size.
    #[test]
    fn int32_figures_are_read_rather_than_dropped() {
        let stats = stats_from_docs(&[doc! {
            "ns": "iMesPyme.pallet",
            "storageStats": { "count": 185_600_i32, "size": 155_685_543_i32, "totalSize": 58_978_304_i32 },
        }]);
        assert_eq!(stats.count, Some(185_600));
        // `totalSize` (data + indexes on disk) wins over the uncompressed `size`.
        assert_eq!(stats.size_bytes, Some(58_978_304));
    }

    #[test]
    fn every_number_width_is_accepted() {
        let stats = stats_from_docs(&[doc! {
            "storageStats": { "count": 5_000_000_000_i64, "storageSize": 1024.0_f64 },
        }]);
        assert_eq!(stats.count, Some(5_000_000_000));
        assert_eq!(stats.size_bytes, Some(1024));
    }

    /// A sharded collection answers with one document per shard; the
    /// collection's figures are the sums, not whichever shard came first.
    #[test]
    fn a_sharded_collection_sums_its_shards() {
        let stats = stats_from_docs(&[
            doc! { "shard": "a", "storageStats": { "count": 10_i32, "totalSize": 100_i32 } },
            doc! { "shard": "b", "storageStats": { "count": 32_i64, "totalSize": 400_i64 } },
        ]);
        assert_eq!(stats.count, Some(42));
        assert_eq!(stats.size_bytes, Some(500));
    }

    /// No figure is not a zero: an unknown size must render as unknown.
    #[test]
    fn a_missing_figure_stays_unknown() {
        assert_eq!(stats_from_docs(&[]), CollectionStats::default());
        let stats = stats_from_docs(&[doc! { "storageStats": { "count": 7_i32 } }]);
        assert_eq!(stats.count, Some(7));
        assert_eq!(stats.size_bytes, None);
    }

    #[test]
    fn negative_or_non_numeric_figures_never_become_huge_unsigned_values() {
        assert_eq!(bson_u64(&Bson::Int32(-1)), Some(0));
        assert_eq!(bson_u64(&Bson::Double(f64::NAN)), None);
        assert_eq!(bson_u64(&Bson::String("12".into())), None);
    }

    // -----------------------------------------------------------------------
    // Classifying a relation from its `listCollections` spec
    //
    // These are what decide whether a describe sends `$sample` and
    // `listIndexes` at all, so each wrong answer has a known cost: a `$sample`
    // that cannot finish, or a `listIndexes` that is a guaranteed error.
    // -----------------------------------------------------------------------

    #[test]
    fn a_view_spec_is_classified_as_a_view() {
        let kind = relation_kind_from_spec(&doc! { "name": "open_orders", "type": "view" });
        assert_eq!(kind, RelationKind::View);
        assert!(!kind.sample_can_be_fast());
        // The case this whole change exists for: asking a view for its indexes
        // fails with code 166 and takes the describe down with it.
        assert!(!kind.has_own_indexes());
    }

    #[test]
    fn a_timeseries_spec_skips_the_sample_but_keeps_its_indexes() {
        let kind = relation_kind_from_spec(&doc! { "name": "readings", "type": "timeseries" });
        assert_eq!(kind, RelationKind::Timeseries);
        // A view over its own buckets, so `$sample` has no fast path either…
        assert!(!kind.sample_can_be_fast());
        // …but unlike a user-defined view it does answer `listIndexes`, which
        // is why the two predicates are not one.
        assert!(kind.has_own_indexes());
    }

    #[test]
    fn an_ordinary_collection_gets_both_fast_paths() {
        let kind = relation_kind_from_spec(&doc! { "name": "orders", "type": "collection" });
        assert_eq!(kind, RelationKind::Collection);
        assert!(kind.sample_can_be_fast());
        assert!(kind.has_own_indexes());
    }

    #[test]
    fn a_spec_with_no_usable_type_is_treated_as_a_collection() {
        // `type` only appeared in MongoDB 3.4, and an unrecognised reply must
        // fall to the ordinary answer — which is also the behaviour of every
        // server old enough not to report one.
        assert_eq!(
            relation_kind_from_spec(&doc! { "name": "orders" }),
            RelationKind::Collection
        );
        assert_eq!(
            relation_kind_from_spec(&doc! { "name": "orders", "type": 1 }),
            RelationKind::Collection
        );
        assert_eq!(
            relation_kind_from_spec(&doc! { "name": "orders", "type": "somethingNew" }),
            RelationKind::Collection
        );
    }
}

// ---------------------------------------------------------------------------
// Name validation
// ---------------------------------------------------------------------------

/// Characters MongoDB refuses in a database name.
///
/// The server's own rule is platform-dependent — `/\. "$*<>:|?` are rejected
/// everywhere, while a space is only rejected on Windows — and a name that is
/// legal on the server but not on every client is a trap rather than a
/// feature, so the strictest set is applied on all platforms. This is the same
/// list Compass enforces client-side.
const MONGO_DB_ILLEGAL: &[char] = &['/', '\\', '.', '"', '$', '*', '<', '>', ':', '|', '?', ' '];

/// MongoDB's hard limit on a database name, in bytes.
///
/// 64 including the terminating null, so 63 usable. Counted in bytes, not
/// characters: the limit is on the encoded name, and a non-ASCII name reaches
/// it sooner than its length suggests.
const MONGO_DB_MAX_BYTES: usize = 63;

/// Validate a database name before it reaches the driver.
///
/// Unlike the SQL drivers this is **not** about escaping — a MongoDB database
/// name is never interpolated into a statement, so there is no quoting to
/// break out of. It exists so the failure is a sentence the user can act on
/// rather than whatever the server says about a namespace it could not parse,
/// and so a name that would be legal today and unusable from a Windows client
/// tomorrow is refused up front.
pub fn validate_database_name(name: &str) -> AppResult<()> {
    if name.is_empty() {
        return Err(AppError::InvalidInput("database name is empty".into()));
    }
    if name.len() > MONGO_DB_MAX_BYTES {
        return Err(AppError::InvalidInput(format!(
            "database name is too long ({} bytes; MongoDB allows {MONGO_DB_MAX_BYTES})",
            name.len()
        )));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| MONGO_DB_ILLEGAL.contains(c) || c.is_control())
    {
        return Err(AppError::InvalidInput(format!(
            "database name contains a character MongoDB does not allow: {bad:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn accepts_ordinary_database_names() {
        for name in ["imespyme", "Dev_Tencer", "reports-2026", "café"] {
            assert!(validate_database_name(name).is_ok(), "rejected {name:?}");
        }
    }

    #[test]
    fn rejects_illegal_database_names() {
        // Empty, every character from the illegal set, and a control byte.
        assert!(validate_database_name("").is_err());
        for bad in [
            "a/b", "a\\b", "a.b", "a\"b", "a$b", "a*b", "a<b", "a>b", "a:b", "a|b", "a?b", "a b",
        ] {
            assert!(validate_database_name(bad).is_err(), "accepted {bad:?}");
        }
        assert!(validate_database_name("a\nb").is_err());
    }

    #[test]
    fn database_name_limit_counts_bytes_not_chars() {
        assert!(validate_database_name(&"a".repeat(MONGO_DB_MAX_BYTES)).is_ok());
        assert!(validate_database_name(&"a".repeat(MONGO_DB_MAX_BYTES + 1)).is_err());
        // 32 two-byte characters is 64 bytes — over the limit at half the length.
        assert!(validate_database_name(&"é".repeat(32)).is_err());
    }

    #[test]
    fn rejects_reserved_collection_names() {
        assert!(validate_collection("").is_err());
        assert!(validate_collection("   ").is_err());
        assert!(validate_collection("system.profile").is_err());
        assert!(validate_collection("a$b").is_err());
        assert!(validate_collection("a\0b").is_err());
        // A dot is legal in a collection name (it is how folders are faked),
        // and the trimmed name is what comes back.
        assert_eq!(validate_collection("  logs.2026 ").unwrap(), "logs.2026");
    }
}
