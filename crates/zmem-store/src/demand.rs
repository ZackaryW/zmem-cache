use super::*;
use zmem_core::demand::{MAX_METADATA_BYTES, MAX_ROUTES, RouteDemand};

pub(crate) fn evict_lru(
    tx: &Transaction<'_>,
    repository: i64,
    head: &str,
    protection: i64,
) -> anyhow::Result<bool> {
    let victim: Option<(i64,String)> = tx.query_row("SELECT p.repository_id,p.head_oid FROM prefetch_jobs p
        LEFT JOIN speculative_cohorts c USING(repository_id,head_oid)
        WHERE p.state IN ('ready','paused_capacity','paused_demand','paused_shutdown')
        AND NOT (p.repository_id=?1 AND p.head_oid=?2)
        AND NOT EXISTS(SELECT 1 FROM index_jobs j JOIN repositories r ON r.id=p.repository_id
            WHERE j.state IN ('queued','running') AND json_valid(j.job_key) AND json_extract(j.job_key,'$.path')=r.path)
        ORDER BY c.last_demand IS NOT NULL,c.last_demand,COALESCE(c.created,0),p.repository_id,p.head_oid LIMIT 1",
        params![repository,head],|row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    let Some((repository, head)) = victim else {
        return Ok(false);
    };
    tx.execute("UPDATE prefetch_jobs SET state='obsolete',completed_count=0,checkpoint_oid=NULL WHERE repository_id=?1 AND head_oid=?2",params![repository,head])?;
    reclaim(tx, protection)?;
    Ok(true)
}

pub(crate) fn reclaim(tx: &Transaction<'_>, protection: i64) -> anyhow::Result<()> {
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS raw_victims(repository_id INTEGER,commit_oid TEXT,PRIMARY KEY(repository_id,commit_oid)); DELETE FROM raw_victims;")?;
    tx.execute("WITH RECURSIVE pinned(repository_id,oid) AS (
        SELECT repository_id,head_oid FROM prefetch_jobs WHERE state NOT IN ('obsolete','failed') UNION
        SELECT e.repository_id,e.parent_oid FROM raw_parent_edges e JOIN pinned p ON p.repository_id=e.repository_id AND p.oid=e.commit_oid)
        INSERT INTO raw_victims SELECT r.repository_id,r.commit_oid FROM raw_commit_facts r
        WHERE NOT EXISTS(SELECT 1 FROM pinned p WHERE p.repository_id=r.repository_id AND p.oid=r.commit_oid)
        AND NOT EXISTS(SELECT 1 FROM trail_membership m WHERE m.repository_id=r.repository_id AND m.commit_oid=r.commit_oid)
        AND (?1=0 OR json_extract(r.fact,'$.commit_time') <= unixepoch()-?1)
        AND NOT EXISTS(SELECT 1 FROM index_jobs j JOIN repositories repo ON repo.id=r.repository_id
            WHERE j.state IN ('queued','running') AND json_valid(j.job_key) AND json_extract(j.job_key,'$.path')=repo.path)", [protection])?;
    tx.execute_batch("UPDATE prefetch_metrics SET
        unused_evicted_bytes=unused_evicted_bytes+(SELECT COALESCE(SUM(u.bytes),0) FROM speculative_usage u JOIN raw_victims v USING(repository_id,commit_oid) WHERE u.first_use IS NULL),
        reclamation_generation=reclamation_generation+EXISTS(SELECT 1 FROM raw_victims) WHERE id=1;
        DELETE FROM raw_parent_edges WHERE EXISTS(SELECT 1 FROM raw_victims v WHERE v.repository_id=raw_parent_edges.repository_id AND v.commit_oid=raw_parent_edges.commit_oid);
        DELETE FROM raw_commit_facts WHERE EXISTS(SELECT 1 FROM raw_victims v WHERE v.repository_id=raw_commit_facts.repository_id AND v.commit_oid=raw_commit_facts.commit_oid);
        DELETE FROM raw_victims;")?;
    Ok(())
}

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS route_demand(
 repository TEXT NOT NULL, route TEXT NOT NULL, payload TEXT NOT NULL, bytes INTEGER NOT NULL,
 PRIMARY KEY(repository,route));
CREATE TABLE IF NOT EXISTS speculative_usage(
 repository_id INTEGER NOT NULL, commit_oid TEXT NOT NULL, bytes INTEGER NOT NULL,
 first_use INTEGER, PRIMARY KEY(repository_id,commit_oid),
 FOREIGN KEY(repository_id,commit_oid) REFERENCES raw_commit_facts(repository_id,commit_oid) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS speculative_cohorts(
 repository_id INTEGER NOT NULL, head_oid TEXT NOT NULL, created INTEGER NOT NULL, last_demand INTEGER,
 PRIMARY KEY(repository_id,head_oid),
 FOREIGN KEY(repository_id,head_oid) REFERENCES prefetch_jobs(repository_id,head_oid) ON DELETE CASCADE);
CREATE INDEX IF NOT EXISTS speculative_lru ON speculative_cohorts(last_demand,created,repository_id,head_oid);
CREATE TABLE IF NOT EXISTS prefetch_metrics(
 id INTEGER PRIMARY KEY CHECK(id=1), produced_facts INTEGER NOT NULL DEFAULT 0,
 produced_bytes INTEGER NOT NULL DEFAULT 0, reused_facts INTEGER NOT NULL DEFAULT 0,
 reused_bytes INTEGER NOT NULL DEFAULT 0, unused_evicted_bytes INTEGER NOT NULL DEFAULT 0,
 reclamation_generation INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO prefetch_metrics(id) VALUES(1);";

pub(crate) fn migrate(connection: &mut Connection) -> anyhow::Result<()> {
    let tx = connection.transaction()?;
    // These names had no meaning in schema 5. User permits replacement of
    // conflicting legacy advisory tables; canonical and failure tables stay.
    tx.execute_batch(
        "DROP TABLE IF EXISTS route_demand;
        DROP TABLE IF EXISTS speculative_usage; DROP TABLE IF EXISTS speculative_cohorts;
        DROP TABLE IF EXISTS prefetch_metrics; PRAGMA defer_foreign_keys=ON;",
    )?;
    tx.execute_batch(SCHEMA)?;
    let records = {
        let mut statement = tx.prepare("SELECT id,repository_id,head_oid,attention_identity,extension_identity FROM trails WHERE schema_version=5 AND protocol_version=5 AND legacy=0")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (old, repository, oid, attention, extension) in records {
        let Some(usage) = zmem_core::AttentionUsage::from_view_identity(&attention) else {
            continue;
        };
        let limit = |value| {
            if value == 0 {
                Ok(zmem_core::AttentionLimit::Zero)
            } else {
                zmem_core::AttentionLimit::parse(value, "migration")
            }
        };
        let identity = TrailIdentity::new(
            repository,
            oid,
            zmem_core::AttentionPolicy {
                commit_limit: limit(usage.commit_limit)?,
                node_limit: limit(usage.node_limit)?,
            },
            extension,
            5,
            6,
        );
        let new = format!("{}:{attention}", identity.key());
        for table in [
            "trail_membership",
            "trail_entry_state",
            "trail_metadata",
            "ref_aliases",
            "relationships",
            "diagnostics",
        ] {
            tx.execute(
                &format!("UPDATE {table} SET trail_id=?1 WHERE trail_id=?2"),
                params![new, old],
            )?;
        }
        tx.execute(
            "UPDATE trails SET id=?1,schema_version=6 WHERE id=?2",
            params![new, old],
        )?;
    }
    tx.execute_batch(
        "UPDATE anchors SET schema_version=6 WHERE schema_version=5; PRAGMA user_version=6;",
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DemandUse {
    pub trail: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default)]
pub struct AdvisoryBatch {
    pub routes: Vec<RouteDemand>,
    pub uses: Vec<DemandUse>,
    pub now: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PrefetchMetrics {
    pub produced_facts: u64,
    pub produced_bytes: u64,
    pub reused_facts: u64,
    pub reused_bytes: u64,
    pub unused_evicted_bytes: u64,
    pub remaining_unused_bytes: u64,
    pub reclamation_generation: u64,
}

impl Store {
    pub fn load_route_demand(&self, now: u64) -> anyhow::Result<Vec<RouteDemand>> {
        let mut rows = self
            .connection
            .prepare("SELECT payload FROM route_demand ORDER BY repository,route LIMIT 4096")?;
        let payloads = rows.query_map([], |row| row.get::<_, String>(0))?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for payload in payloads {
            let payload = payload?;
            bytes += payload.len();
            if bytes > MAX_METADATA_BYTES / 2 {
                break;
            }
            if let Ok(mut demand) = serde_json::from_str::<RouteDemand>(&payload) {
                demand.expire(now);
                if !demand.observations.is_empty() {
                    result.push(demand);
                }
            }
        }
        Ok(result)
    }

    pub fn persist_advisory(&mut self, batch: &AdvisoryBatch) -> anyhow::Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM route_demand", [])?;
        let mut bytes = 0;
        for route in batch.routes.iter().take(MAX_ROUTES) {
            let mut route = route.clone();
            route.expire(batch.now);
            if route.observations.is_empty() {
                continue;
            }
            let payload = serde_json::to_string(&route)?;
            bytes += payload.len();
            if bytes > MAX_METADATA_BYTES {
                break;
            }
            tx.execute(
                "INSERT OR REPLACE INTO route_demand VALUES(?1,?2,?3,?4)",
                params![route.repository, route.route, payload, payload.len()],
            )?;
        }
        for usage in &batch.uses {
            record_use(&tx, &usage.trail, usage.at)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn prefetch_metrics(&self) -> anyhow::Result<PrefetchMetrics> {
        Ok(self.connection.query_row("SELECT produced_facts,produced_bytes,reused_facts,reused_bytes,unused_evicted_bytes,reclamation_generation,
            (SELECT COALESCE(SUM(bytes),0) FROM speculative_usage WHERE first_use IS NULL) FROM prefetch_metrics WHERE id=1",[],|row| Ok(PrefetchMetrics {
            produced_facts:row.get(0)?,produced_bytes:row.get(1)?,reused_facts:row.get(2)?,reused_bytes:row.get(3)?,unused_evicted_bytes:row.get(4)?,reclamation_generation:row.get(5)?,remaining_unused_bytes:row.get(6)?
        }))?)
    }
}

pub(crate) fn record_use(tx: &Transaction<'_>, trail: &str, at: u64) -> anyhow::Result<()> {
    tx.execute("UPDATE prefetch_metrics SET
        reused_facts=reused_facts+(SELECT COUNT(*) FROM speculative_usage u JOIN trail_membership m USING(repository_id,commit_oid) WHERE m.trail_id=?1 AND u.first_use IS NULL),
        reused_bytes=reused_bytes+(SELECT COALESCE(SUM(u.bytes),0) FROM speculative_usage u JOIN trail_membership m USING(repository_id,commit_oid) WHERE m.trail_id=?1 AND u.first_use IS NULL) WHERE id=1",[trail])?;
    tx.execute("UPDATE speculative_usage SET first_use=?2 WHERE first_use IS NULL AND EXISTS(SELECT 1 FROM trail_membership m WHERE m.trail_id=?1 AND m.repository_id=speculative_usage.repository_id AND m.commit_oid=speculative_usage.commit_oid)",params![trail,at])?;
    tx.execute("WITH RECURSIVE members(repository_id,head_oid,oid) AS (
        SELECT repository_id,head_oid,head_oid FROM speculative_cohorts UNION
        SELECT m.repository_id,m.head_oid,e.parent_oid FROM members m JOIN raw_parent_edges e ON e.repository_id=m.repository_id AND e.commit_oid=m.oid)
        UPDATE speculative_cohorts SET last_demand=MAX(COALESCE(last_demand,0),?2) WHERE EXISTS(
        SELECT 1 FROM members m JOIN trail_membership t ON t.repository_id=m.repository_id AND t.commit_oid=m.oid
        WHERE t.trail_id=?1 AND m.repository_id=speculative_cohorts.repository_id AND m.head_oid=speculative_cohorts.head_oid
        AND EXISTS(SELECT 1 FROM speculative_usage u WHERE u.repository_id=m.repository_id AND u.commit_oid=m.oid))",params![trail,at])?;
    Ok(())
}
