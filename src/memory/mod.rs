use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::{agents, db, openrouter};

const CURATOR_SYSTEM_PROMPT: &str = r#"You maintain long-term memory for a single user conversation stream.
Return JSON only with this exact shape:
{
  "updates": [
    {
      "statement": "short canonical statement about the user",
      "delta": -1.0 to 1.0,
      "salience": 0.0 to 1.0,
      "confidence": 0.0 to 1.0,
      "evidence": "very short reason"
    }
  ],
  "links": [
    {
      "from": "statement text",
      "to": "statement text",
      "kind": "supports|conflicts|related",
      "strength": 0.0 to 1.0
    }
  ]
}
Rules:
- Use concise neutral wording.
- Create updates only when there is meaningful memory signal.
- Repetition should increase positive delta for the same meaning.
- Contradiction should emit negative delta.
- Do not include prose outside JSON."#;

#[derive(Debug, Clone)]
struct ScoredStatement {
    statement: db::MemoryStatementRecord,
    relevance: f64,
    score: f64,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorPayload {
    #[serde(default)]
    updates: Vec<CuratorUpdate>,
    #[serde(default)]
    links: Vec<CuratorLink>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorUpdate {
    #[serde(default, alias = "text", alias = "claim")]
    statement: String,
    #[serde(default, alias = "weight", alias = "score_delta")]
    delta: f64,
    #[serde(default)]
    salience: f64,
    #[serde(default)]
    confidence: f64,
    #[serde(default, alias = "reason")]
    evidence: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct CuratorLink {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    strength: f64,
}

const SUPERVISOR_SLEEP: Duration = Duration::from_millis(1200);
const USER_WORKER_IDLE_SLEEP: Duration = Duration::from_millis(300);
const USER_WORKER_IDLE_TICKS_BEFORE_EXIT: usize = 6;
const EMBEDDING_BACKFILL_BATCH_SIZE: usize = 12;
const SEMANTIC_THREAD_PRIMARY_THRESHOLD: f64 = 0.70;
const SEMANTIC_THREAD_SECONDARY_THRESHOLD: f64 = 0.60;
const SEMANTIC_THREAD_MERGE_THRESHOLD: f64 = 0.93;
const SEMANTIC_THREAD_MAX_SECONDARY: usize = 2;
const SEMANTIC_THREAD_NEAREST_K: usize = 8;
const SEMANTIC_THREAD_PROJECTION_DIMS: usize = 192;
const SEMANTIC_THREAD_CANDIDATES: usize = 96;
const SEMANTIC_THREAD_ANN_TABLES: usize = 6;
const SEMANTIC_THREAD_ANN_BITS: usize = 8;
const SEMANTIC_THREAD_ANN_MIN_THREADS: usize = 256;
const SEMANTIC_THREAD_ANN_MAX_CANDIDATES: usize = 384;
const SEMANTIC_THREAD_FLUSH_EVERY_UPDATES: usize = 96;
const SEMANTIC_THREAD_FLUSH_INTERVAL: Duration = Duration::from_secs(2);
static INGEST_LOOP_STARTED: OnceLock<()> = OnceLock::new();
static ACTIVE_USER_WORKERS: OnceLock<Mutex<HashMap<String, ()>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct CachedSemanticThread {
    id: i64,
    message_count: i64,
    centroid: Vec<f32>,
    centroid_unit: Vec<f32>,
    centroid_proj_unit: Vec<f32>,
    dirty: bool,
}

#[derive(Debug)]
struct SemanticAnnIndex {
    dims: usize,
    planes: Vec<Vec<Vec<f32>>>,
    buckets: Vec<HashMap<u64, Vec<usize>>>,
    keys_by_thread: Vec<Vec<u64>>,
}

#[derive(Debug)]
struct SemanticThreadCache {
    user_id: String,
    model: String,
    dims: usize,
    threads: Vec<CachedSemanticThread>,
    by_id: HashMap<i64, usize>,
    ann: Option<SemanticAnnIndex>,
    pending_updates: usize,
    last_flush: Instant,
}

#[derive(Debug, Default)]
struct SemanticThreadRuntime {
    current: Option<SemanticThreadCache>,
}

fn active_user_workers() -> &'static Mutex<HashMap<String, ()>> {
    ACTIVE_USER_WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

impl SemanticThreadRuntime {
    fn ensure_loaded(
        &mut self,
        data_dir: &PathBuf,
        uds: &db::UserDataStore,
        user_id: &str,
        model: &str,
        dims: usize,
    ) -> anyhow::Result<&mut SemanticThreadCache> {
        let needs_reload = match self.current.as_ref() {
            Some(cache) => cache.user_id != user_id || cache.model != model || cache.dims != dims,
            None => true,
        };
        if needs_reload {
            self.flush_current(data_dir, true)?;
            self.current = Some(SemanticThreadCache::load(uds, user_id, model, dims)?);
        }
        Ok(self.current.as_mut().expect("cache must be loaded"))
    }

    fn flush_current(&mut self, data_dir: &PathBuf, force: bool) -> anyhow::Result<()> {
        let should_flush = self
            .current
            .as_ref()
            .map(|c| c.should_flush(force))
            .unwrap_or(false);
        if !should_flush {
            return Ok(());
        }
        let user_id = match self.current.as_ref() {
            Some(c) => c.user_id.clone(),
            None => return Ok(()),
        };
        let mgr = db::Manager::new(data_dir.clone());
        let uds = mgr.user_data(&user_id)?;
        if let Some(cache) = self.current.as_mut() {
            cache.flush(&uds, force)?;
        }
        Ok(())
    }

    fn invalidate_current(&mut self) {
        self.current = None;
    }
}

impl SemanticAnnIndex {
    fn build(threads: &[CachedSemanticThread]) -> Option<Self> {
        if threads.len() < SEMANTIC_THREAD_ANN_MIN_THREADS {
            return None;
        }
        let dims = threads
            .first()
            .map(|t| t.centroid_proj_unit.len())
            .unwrap_or(0);
        if dims == 0 {
            return None;
        }

        let mut planes = Vec::with_capacity(SEMANTIC_THREAD_ANN_TABLES);
        for table in 0..SEMANTIC_THREAD_ANN_TABLES {
            let mut table_planes = Vec::with_capacity(SEMANTIC_THREAD_ANN_BITS);
            for bit in 0..SEMANTIC_THREAD_ANN_BITS {
                let mut plane = Vec::with_capacity(dims);
                for dim in 0..dims {
                    plane.push(ann_plane_coeff(table, bit, dim));
                }
                table_planes.push(plane);
            }
            planes.push(table_planes);
        }

        let mut out = Self {
            dims,
            planes,
            buckets: (0..SEMANTIC_THREAD_ANN_TABLES)
                .map(|_| HashMap::new())
                .collect(),
            keys_by_thread: vec![vec![0_u64; SEMANTIC_THREAD_ANN_TABLES]; threads.len()],
        };
        for (idx, thread) in threads.iter().enumerate() {
            out.on_thread_added(idx, &thread.centroid_proj_unit);
        }
        Some(out)
    }

    fn query(
        &self,
        query_proj: &[f32],
        min_candidates: usize,
        max_candidates: usize,
    ) -> Vec<usize> {
        if query_proj.len() != self.dims || self.dims == 0 {
            return Vec::new();
        }
        let mut keys = vec![0_u64; SEMANTIC_THREAD_ANN_TABLES];
        for table in 0..SEMANTIC_THREAD_ANN_TABLES {
            keys[table] = self.table_key(table, query_proj);
        }

        let target = min_candidates.max(1);
        let cap = max_candidates.max(target);
        let mut votes: HashMap<usize, u16> = HashMap::new();
        for (table, key) in keys.iter().copied().enumerate() {
            if let Some(bucket) = self.buckets[table].get(&key) {
                add_ann_votes(&mut votes, bucket);
            }
        }

        if votes.len() < target {
            for (table, key) in keys.iter().copied().enumerate() {
                for bit in 0..SEMANTIC_THREAD_ANN_BITS {
                    if votes.len() >= target {
                        break;
                    }
                    let neighbor = key ^ (1_u64 << bit);
                    if let Some(bucket) = self.buckets[table].get(&neighbor) {
                        add_ann_votes(&mut votes, bucket);
                    }
                }
                if votes.len() >= target {
                    break;
                }
            }
        }

        let mut ranked = votes.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(cap);
        ranked.into_iter().map(|(idx, _)| idx).collect()
    }

    fn on_thread_added(&mut self, idx: usize, vec: &[f32]) {
        if vec.len() != self.dims {
            return;
        }
        if idx >= self.keys_by_thread.len() {
            self.keys_by_thread
                .resize(idx + 1, vec![0_u64; SEMANTIC_THREAD_ANN_TABLES]);
        }
        for table in 0..SEMANTIC_THREAD_ANN_TABLES {
            let key = self.table_key(table, vec);
            self.keys_by_thread[idx][table] = key;
            self.buckets[table].entry(key).or_default().push(idx);
        }
    }

    fn on_thread_updated(&mut self, idx: usize, vec: &[f32]) {
        if vec.len() != self.dims || idx >= self.keys_by_thread.len() {
            return;
        }
        for table in 0..SEMANTIC_THREAD_ANN_TABLES {
            let old_key = self.keys_by_thread[idx][table];
            let new_key = self.table_key(table, vec);
            if old_key == new_key {
                continue;
            }
            if let Some(bucket) = self.buckets[table].get_mut(&old_key) {
                remove_ann_index(bucket, idx);
            }
            self.buckets[table].entry(new_key).or_default().push(idx);
            self.keys_by_thread[idx][table] = new_key;
        }
    }

    fn table_key(&self, table: usize, vec: &[f32]) -> u64 {
        let mut key = 0_u64;
        for bit in 0..SEMANTIC_THREAD_ANN_BITS {
            let plane = &self.planes[table][bit];
            let dot = dot_product_simd(vec, plane);
            if dot >= 0.0 {
                key |= 1_u64 << bit;
            }
        }
        key
    }
}

impl SemanticThreadCache {
    fn load(
        uds: &db::UserDataStore,
        user_id: &str,
        model: &str,
        dims: usize,
    ) -> anyhow::Result<Self> {
        let mut threads = Vec::new();
        let mut by_id = HashMap::new();
        for rec in uds
            .list_semantic_threads(model, dims)
            .context("list semantic threads")?
        {
            let centroid_unit = normalize_unit_vector(&rec.centroid);
            if centroid_unit.is_empty() {
                continue;
            }
            let centroid_proj_unit =
                project_and_normalize(&rec.centroid, SEMANTIC_THREAD_PROJECTION_DIMS);
            if centroid_proj_unit.is_empty() {
                continue;
            }
            let idx = threads.len();
            by_id.insert(rec.id, idx);
            threads.push(CachedSemanticThread {
                id: rec.id,
                message_count: rec.message_count.max(0),
                centroid: rec.centroid,
                centroid_unit,
                centroid_proj_unit,
                dirty: false,
            });
        }
        let ann = SemanticAnnIndex::build(&threads);
        Ok(Self {
            user_id: user_id.to_string(),
            model: model.to_string(),
            dims,
            threads,
            by_id,
            ann,
            pending_updates: 0,
            last_flush: Instant::now(),
        })
    }

    fn nearest(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        min_similarity: f64,
    ) -> Vec<(i64, f64)> {
        let query_unit = normalize_unit_vector(query_embedding);
        if query_unit.is_empty() || top_k == 0 {
            return Vec::new();
        }
        if self.threads.is_empty() {
            return Vec::new();
        }

        let query_proj = project_and_normalize(query_embedding, SEMANTIC_THREAD_PROJECTION_DIMS);
        let candidate_target = self
            .threads
            .len()
            .min(SEMANTIC_THREAD_CANDIDATES.max(top_k.saturating_mul(8)));

        let mut candidate_idxs = if query_proj.is_empty() {
            self.threads
                .iter()
                .enumerate()
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>()
        } else if let Some(ann) = &self.ann {
            ann.query(
                &query_proj,
                candidate_target,
                candidate_target.max(SEMANTIC_THREAD_ANN_MAX_CANDIDATES),
            )
        } else {
            self.projected_top_candidates(&query_proj, candidate_target)
        };

        if candidate_idxs.len() < top_k {
            for idx in self.projected_top_candidates(&query_proj, candidate_target) {
                if candidate_idxs.contains(&idx) {
                    continue;
                }
                candidate_idxs.push(idx);
                if candidate_idxs.len() >= candidate_target {
                    break;
                }
            }
        }
        if candidate_idxs.is_empty() {
            candidate_idxs = (0..self.threads.len()).collect();
        }

        let mut scored = Vec::new();
        for idx in candidate_idxs {
            let thread = &self.threads[idx];
            let sim = f64::from(dot_product_simd(&query_unit, &thread.centroid_unit))
                .clamp(-1.0, 1.0)
                .max(0.0);
            if sim >= min_similarity {
                scored.push((thread.id, sim));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k);
        scored
    }

    fn add_thread(&mut self, thread_id: i64, embedding: &[f32]) {
        if thread_id <= 0 || embedding.is_empty() {
            return;
        }
        let unit = normalize_unit_vector(embedding);
        if unit.is_empty() {
            return;
        }
        let idx = self.threads.len();
        self.by_id.insert(thread_id, idx);
        self.threads.push(CachedSemanticThread {
            id: thread_id,
            message_count: 0,
            centroid: embedding.to_vec(),
            centroid_unit: unit,
            centroid_proj_unit: project_and_normalize(embedding, SEMANTIC_THREAD_PROJECTION_DIMS),
            dirty: false,
        });
        if self.ann.is_none() {
            self.ann = SemanticAnnIndex::build(&self.threads);
        } else if let Some(ann) = self.ann.as_mut() {
            ann.on_thread_added(idx, &self.threads[idx].centroid_proj_unit);
        }
    }

    fn apply_membership_insert(&mut self, thread_id: i64, embedding: &[f32]) {
        if embedding.is_empty() {
            return;
        }
        let Some(idx) = self.by_id.get(&thread_id).copied() else {
            return;
        };
        let thread = &mut self.threads[idx];
        if thread.centroid.len() != embedding.len() {
            return;
        }
        let base = thread.message_count.max(0) as f32;
        let denom = base + 1.0;
        for (curr, next) in thread.centroid.iter_mut().zip(embedding.iter()) {
            *curr = ((*curr * base) + *next) / denom;
        }
        thread.message_count = thread.message_count.saturating_add(1);
        thread.centroid_unit = normalize_unit_vector(&thread.centroid);
        thread.centroid_proj_unit =
            project_and_normalize(&thread.centroid, SEMANTIC_THREAD_PROJECTION_DIMS);
        if let Some(ann) = self.ann.as_mut() {
            ann.on_thread_updated(idx, &thread.centroid_proj_unit);
        }
        if !thread.dirty {
            thread.dirty = true;
        }
        self.pending_updates = self.pending_updates.saturating_add(1);
    }

    fn should_flush(&self, force: bool) -> bool {
        if self.pending_updates == 0 || !self.threads.iter().any(|t| t.dirty) {
            return false;
        }
        force
            || self.pending_updates >= SEMANTIC_THREAD_FLUSH_EVERY_UPDATES
            || self.last_flush.elapsed() >= SEMANTIC_THREAD_FLUSH_INTERVAL
    }

    fn flush(&mut self, uds: &db::UserDataStore, force: bool) -> anyhow::Result<()> {
        if !self.should_flush(force) {
            return Ok(());
        }
        let mut updates = Vec::new();
        for thread in &self.threads {
            if thread.dirty {
                updates.push((thread.id, thread.centroid.clone(), thread.message_count));
            }
        }
        if !updates.is_empty() {
            uds.update_semantic_thread_centroids_batch(&updates)
                .context("flush semantic thread centroids")?;
        }
        for thread in &mut self.threads {
            if thread.dirty {
                thread.dirty = false;
            }
        }
        self.pending_updates = 0;
        self.last_flush = Instant::now();
        Ok(())
    }

    fn projected_top_candidates(&self, query_proj: &[f32], limit: usize) -> Vec<usize> {
        if query_proj.is_empty() {
            return Vec::new();
        }
        let mut scored = self
            .threads
            .iter()
            .enumerate()
            .map(|(idx, thread)| {
                let sim = f64::from(dot_product_simd(query_proj, &thread.centroid_proj_unit))
                    .clamp(-1.0, 1.0)
                    .max(0.0);
                (idx, sim)
            })
            .collect::<Vec<_>>();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit.max(1));
        scored.into_iter().map(|(idx, _)| idx).collect()
    }
}

pub fn spawn_ingest_user_message(
    data_dir: PathBuf,
    user_id: String,
    _conversation_id: String,
    message_id: i64,
) {
    let mgr = db::Manager::new(data_dir.clone());
    match mgr.user_data(&user_id) {
        Ok(uds) => {
            if let Err(e) = uds.queue_message_ingest(message_id) {
                warn!(err=%e, user_id=%user_id, message_id=message_id, "memory ingest queue failed");
            }
        }
        Err(e) => {
            warn!(err=%e, user_id=%user_id, message_id=message_id, "memory ingest user db open failed");
        }
    }
    ensure_backfill_worker(data_dir.clone());
    let kickoff_user_id = user_id.clone();
    tokio::spawn(async move {
        if let Err(e) = ensure_user_worker_for(data_dir, kickoff_user_id).await {
            warn!(err=%e, "memory ingest user worker ensure failed");
        }
    });
}

pub fn ensure_backfill_worker(data_dir: PathBuf) {
    INGEST_LOOP_STARTED.get_or_init(|| {
        recover_stuck_ingest_jobs(&data_dir);
        tokio::spawn(async move {
            loop {
                if let Err(e) = ensure_workers_for_pending_users(&data_dir).await {
                    warn!(err=%e, "memory backfill scan failed");
                }
                tokio::time::sleep(SUPERVISOR_SLEEP).await;
            }
        });
    });
}

async fn ensure_workers_for_pending_users(data_dir: &PathBuf) -> anyhow::Result<()> {
    let mgr = db::Manager::new(data_dir.clone());
    let users = mgr.users()?.list_users()?;
    for u in users {
        let uds = match mgr.user_data(&u.id) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, user_id=%u.id, "memory backfill: user db open failed");
                continue;
            }
        };
        if uds.next_backfill_message()?.is_none() {
            continue;
        }
        ensure_user_worker_for(data_dir.clone(), u.id).await?;
    }
    Ok(())
}

async fn ensure_user_worker_for(data_dir: PathBuf, user_id: String) -> anyhow::Result<()> {
    {
        let active = active_user_workers().lock().await;
        if active.contains_key(&user_id) {
            return Ok(());
        }
    }

    {
        let mgr = db::Manager::new(data_dir.clone());
        let uds = mgr.user_data(&user_id)?;
        if uds.next_backfill_message()?.is_none() {
            return Ok(());
        }
    }

    {
        let mut active = active_user_workers().lock().await;
        if active.contains_key(&user_id) {
            return Ok(());
        }
        active.insert(user_id.clone(), ());
    }

    tokio::spawn(user_worker_loop(data_dir, user_id));
    Ok(())
}

async fn user_worker_loop(data_dir: PathBuf, user_id: String) {
    let mut thread_runtime = SemanticThreadRuntime::default();
    let mut idle_ticks = 0usize;

    loop {
        let next = match next_user_backfill_job(&data_dir, &user_id) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, user_id=%user_id, "memory backfill: user scan failed");
                break;
            }
        };

        let Some((message_id, conversation_id)) = next else {
            if let Err(flush_err) = thread_runtime.flush_current(&data_dir, false) {
                warn!(err=%flush_err, user_id=%user_id, "semantic thread cache flush failed");
            }
            idle_ticks = idle_ticks.saturating_add(1);
            if idle_ticks >= USER_WORKER_IDLE_TICKS_BEFORE_EXIT {
                break;
            }
            tokio::time::sleep(USER_WORKER_IDLE_SLEEP).await;
            continue;
        };

        idle_ticks = 0;
        if let Err(e) = ingest_user_message(
            data_dir.clone(),
            user_id.clone(),
            conversation_id.to_string(),
            message_id,
            &mut thread_runtime,
        )
        .await
        {
            warn!(
                err = %e,
                user_id = %user_id,
                message_id = message_id,
                "memory ingest failed"
            );
        }
    }

    if let Err(flush_err) = thread_runtime.flush_current(&data_dir, true) {
        warn!(err=%flush_err, user_id=%user_id, "semantic thread cache flush failed");
    }

    let mut active = active_user_workers().lock().await;
    active.remove(&user_id);
}

fn next_user_backfill_job(data_dir: &PathBuf, user_id: &str) -> anyhow::Result<Option<(i64, i64)>> {
    let mgr = db::Manager::new(data_dir.clone());
    let uds = mgr.user_data(user_id)?;
    uds.next_backfill_message().map_err(anyhow::Error::from)
}

fn recover_stuck_ingest_jobs(data_dir: &PathBuf) {
    let mgr = db::Manager::new(data_dir.clone());
    let users = match mgr.users() {
        Ok(us) => match us.list_users() {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, "memory backfill: failed to list users for ingest recovery");
                return;
            }
        },
        Err(e) => {
            warn!(err=%e, "memory backfill: failed to open users db for ingest recovery");
            return;
        }
    };

    let mut total = 0usize;
    for u in users {
        let uds = match mgr.user_data(&u.id) {
            Ok(v) => v,
            Err(e) => {
                warn!(err=%e, user_id=%u.id, "memory backfill: user db open failed during ingest recovery");
                continue;
            }
        };
        match uds.recover_running_ingest_to_queued() {
            Ok(n) => total += n,
            Err(e) => warn!(err=%e, user_id=%u.id, "memory backfill: ingest recovery failed"),
        }
    }

    if total > 0 {
        info!(
            recovered_jobs = total,
            "memory backfill: recovered stuck running ingest jobs"
        );
    }
}

pub async fn build_memory_context(
    data_dir: PathBuf,
    user_id: String,
    conversation_id: String,
    latest_user_text: String,
) -> Option<String> {
    match build_memory_context_inner(data_dir, user_id, conversation_id, latest_user_text).await {
        Ok(v) => v,
        Err(e) => {
            warn!(err=%e, "memory context build failed");
            None
        }
    }
}

async fn build_memory_context_inner(
    data_dir: PathBuf,
    user_id: String,
    conversation_id: String,
    latest_user_text: String,
) -> anyhow::Result<Option<String>> {
    let query_text = latest_user_text.trim();
    if query_text.is_empty() {
        return Ok(None);
    }

    let embed_backend = agents::client_for_task_from_disk(&data_dir, "memory")?;
    let query_embedding =
        embed_single_text(&embed_backend.client, &embed_backend.model, query_text)
            .await
            .context("embed query")?;

    let mgr = db::Manager::new(data_dir);
    let uds = mgr.user_data(&user_id)?;
    let conv_id = parse_conversation_id(&conversation_id).or(Some(1));
    let nearest =
        uds.nearest_memory_statements(&embed_backend.model, &query_embedding, 80, conv_id)?;
    if nearest.is_empty() {
        return Ok(None);
    }

    let mut ranked: Vec<ScoredStatement> = nearest
        .into_iter()
        .filter_map(|(statement, distance)| {
            let relevance = (1.0 - distance).clamp(-1.0, 1.0).max(0.0);
            if relevance < 0.10 {
                return None;
            }
            let belief_strength = belief_strength(statement.belief_score);
            let salience = statement.salience.clamp(0.0, 1.0);
            let score = (relevance * 0.70) + (belief_strength * 0.20) + (salience * 0.10);
            Some(ScoredStatement {
                statement,
                relevance,
                score,
            })
        })
        .collect();

    if ranked.is_empty() {
        return Ok(None);
    }

    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked.truncate(8);

    let mut lines = Vec::with_capacity(ranked.len() + 2);
    lines.push("Memory hints (probabilistic; may be stale):".to_string());
    for s in ranked {
        lines.push(format!(
            "S{} belief={:.2} rel={:.2} sal={:.2} evidence={} first={} last={} text={}",
            s.statement.id,
            s.statement.belief_score,
            s.relevance,
            s.statement.salience,
            s.statement.evidence_count,
            s.statement.first_seen_message_id,
            s.statement.last_seen_message_id,
            s.statement.text
        ));
    }
    lines.push("Use memory to maintain continuity. Do not assume memory is always true; treat it as weighted signal.".to_string());

    Ok(Some(lines.join("\n")))
}

async fn ingest_user_message(
    data_dir: PathBuf,
    user_id: String,
    conversation_id: String,
    message_id: i64,
    thread_runtime: &mut SemanticThreadRuntime,
) -> anyhow::Result<()> {
    let mgr = db::Manager::new(data_dir.clone());
    let uds = mgr.user_data(&user_id)?;
    if !uds.claim_message_ingest(message_id)? {
        return Ok(());
    }

    let fast_path = (|| -> anyhow::Result<bool> {
        let message = message_for_ingest(&uds, message_id)?;
        let Some(message) = message else {
            uds.complete_message_ingest(message_id)?;
            return Ok(true);
        };

        let target_conversation =
            parse_conversation_id(&conversation_id).unwrap_or(message.conversation_id);
        if message.conversation_id != target_conversation {
            uds.complete_message_ingest(message_id)?;
            return Ok(true);
        }

        let existing = match uds.message_embedding(message_id)? {
            Some(v) => v,
            None => return Ok(false),
        };

        let embed_model = if let Some(model) = existing.model.and_then(|m| {
            let t = m.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }) {
            model
        } else {
            agents::client_for_task_from_disk(&data_dir, "memory")
                .map(|b| b.model)
                .unwrap_or_else(|_| "openai/text-embedding-3-large".to_string())
        };

        assign_semantic_threads(
            &data_dir,
            thread_runtime,
            &uds,
            &user_id,
            &embed_model,
            message_id,
            &existing.embedding,
        )
        .context("assign semantic threads (fast path)")?;

        uds.complete_message_ingest(message_id)?;
        if curator_disabled_for_now() {
            info!(
                user_id = %user_id,
                message_id = message_id,
                model = %embed_model,
                "memory ingest: curator disabled, embedding stored"
            );
        }
        Ok(true)
    })();

    match fast_path {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => {
            let msg = format!("{e:#}");
            if let Err(mark_err) = uds.fail_message_ingest(message_id, &msg) {
                warn!(err=%mark_err, message_id=message_id, "memory ingest failure state update failed");
            }
            return Err(e);
        }
    }
    drop(uds);

    let ingest_res = ingest_user_message_inner(
        data_dir.clone(),
        user_id.clone(),
        conversation_id,
        message_id,
        thread_runtime,
    )
    .await;

    match ingest_res {
        Ok(()) => {
            let uds = mgr.user_data(&user_id)?;
            uds.complete_message_ingest(message_id)?;
            Ok(())
        }
        Err(e) => {
            let uds = mgr.user_data(&user_id)?;
            let msg = format!("{e:#}");
            if let Err(mark_err) = uds.fail_message_ingest(message_id, &msg) {
                warn!(err=%mark_err, message_id=message_id, "memory ingest failure state update failed");
            }
            Err(e)
        }
    }
}

async fn ingest_user_message_inner(
    data_dir: PathBuf,
    user_id: String,
    conversation_id: String,
    message_id: i64,
    thread_runtime: &mut SemanticThreadRuntime,
) -> anyhow::Result<()> {
    let mgr = db::Manager::new(data_dir.clone());
    let uds = mgr.user_data(&user_id)?;
    let message = message_for_ingest(&uds, message_id)?;
    let Some(message) = message else {
        return Ok(());
    };
    let target_conversation =
        parse_conversation_id(&conversation_id).unwrap_or(message.conversation_id);
    if message.conversation_id != target_conversation {
        return Ok(());
    }

    let mut embed_backend: Option<agents::ResolvedTaskClient> = None;
    let mut get_embed_backend = || -> anyhow::Result<agents::ResolvedTaskClient> {
        if let Some(b) = &embed_backend {
            return Ok(b.clone());
        }
        let b = agents::client_for_task_from_disk(&data_dir, "memory")?;
        embed_backend = Some(b.clone());
        Ok(b)
    };

    let existing_embedding = uds.message_embedding(message_id)?;
    let (embed_model, msg_embedding) = if let Some(existing) = existing_embedding {
        let model = match existing.model.and_then(|m| {
            let t = m.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }) {
            Some(m) => m,
            None => get_embed_backend()?.model.clone(),
        };
        (model, existing.embedding)
    } else if curator_disabled_for_now() {
        let backend = get_embed_backend()?;
        let rows = uds
            .list_backfill_messages_without_embedding_from(
                message_id,
                EMBEDDING_BACKFILL_BATCH_SIZE,
            )
            .context("list backfill embedding batch")?;
        if rows.is_empty() {
            let emb = embed_single_text(&backend.client, &backend.model, &message.content)
                .await
                .context("embed message fallback")?;
            uds.upsert_message_embedding(message_id, &backend.model, &emb)?;
            (backend.model.clone(), emb)
        } else {
            let inputs = rows.iter().map(|m| m.content.clone()).collect::<Vec<_>>();
            let vectors = embed_many_texts(&backend.client, &backend.model, &inputs)
                .await
                .context("embed message batch")?;

            let mut current = None;
            for (row, emb) in rows.iter().zip(vectors.iter()) {
                uds.upsert_message_embedding(row.id, &backend.model, emb)?;
                if row.id == message_id {
                    current = Some(emb.clone());
                }
            }

            if let Some(v) = current {
                (backend.model.clone(), v)
            } else if let Some(existing) = uds.message_embedding(message_id)? {
                let model = existing
                    .model
                    .and_then(|m| {
                        let t = m.trim();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t.to_string())
                        }
                    })
                    .unwrap_or_else(|| backend.model.clone());
                (model, existing.embedding)
            } else {
                let emb = embed_single_text(&backend.client, &backend.model, &message.content)
                    .await
                    .context("embed message final fallback")?;
                uds.upsert_message_embedding(message_id, &backend.model, &emb)?;
                (backend.model.clone(), emb)
            }
        }
    } else {
        let backend = get_embed_backend()?;
        let emb = embed_single_text(&backend.client, &backend.model, &message.content)
            .await
            .context("embed message")?;
        uds.upsert_message_embedding(message_id, &backend.model, &emb)?;
        (backend.model.clone(), emb)
    };

    assign_semantic_threads(
        &data_dir,
        thread_runtime,
        &uds,
        &user_id,
        &embed_model,
        message_id,
        &msg_embedding,
    )
    .context("assign semantic threads")?;

    // While curator is disabled, continue backfill by storing embeddings only.
    if curator_disabled_for_now() {
        info!(
            user_id = %user_id,
            message_id = message_id,
            model = %embed_model,
            "memory ingest: curator disabled, embedding stored"
        );
        return Ok(());
    }

    let embed_backend = get_embed_backend()?;

    // Backfill is strictly oldest->newest; cap context to <= current message id.
    let recent_rows = uds
        .list_recent_messages_for_conversation_up_to(
            &message.conversation_id.to_string(),
            24,
            Some(message_id),
        )
        .unwrap_or_default();

    let similar_rows = uds
        .nearest_messages_by_embedding(
            &embed_backend.model,
            &msg_embedding,
            message_id,
            Some(target_conversation),
            Some(message_id),
            8,
            0.30,
        )
        .unwrap_or_default()
        .into_iter()
        .map(|m| format!("{}:{}: {}", m.id, m.role, sanitize_line(&m.content)))
        .collect::<Vec<_>>();

    let known_statements = uds
        .list_memory_statements(80, Some(&embed_backend.model), Some(target_conversation))
        .unwrap_or_default()
        .into_iter()
        .take(12)
        .map(|s| {
            format!(
                "S{} belief={:.2} sal={:.2} evidence={} text={}",
                s.id,
                s.belief_score,
                s.salience,
                s.evidence_count,
                sanitize_line(&s.text)
            )
        })
        .collect::<Vec<_>>();

    let recent_text = recent_rows
        .iter()
        .map(|m| format!("{}:{}: {}", m.id, m.role, sanitize_line(&m.content)))
        .collect::<Vec<_>>()
        .join("\n");

    let curator_backend = agents::client_for_task_from_disk(&data_dir, "memory")?;
    let curator_temperature = curator_backend
        .settings
        .resolve_agent("memory")
        .and_then(|a| a.normalized_temperature())
        .unwrap_or(0.0);
    let curator_input = format!(
        "New message:\n{}:{}\n\nRecent conversation window:\n{}\n\nSemantically related older messages:\n{}\n\nKnown memory statements:\n{}\n",
        message.id,
        sanitize_line(&message.content),
        recent_text,
        if similar_rows.is_empty() {
            "(none)".to_string()
        } else {
            similar_rows.join("\n")
        },
        if known_statements.is_empty() {
            "(none)".to_string()
        } else {
            known_statements.join("\n")
        }
    );

    let req = openrouter::ChatCompletionRequest {
        model: curator_backend.model.clone(),
        messages: vec![
            openrouter::Message {
                role: "system".to_string(),
                content: CURATOR_SYSTEM_PROMPT.to_string(),
            },
            openrouter::Message {
                role: "user".to_string(),
                content: curator_input,
            },
        ],
        temperature: Some(curator_temperature),
        max_tokens: Some(700),
        stream: false,
    };

    let raw = curator_backend
        .client
        .create_chat_completion(&req)
        .await
        .context("curator completion failed")?
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default();

    let payload = parse_curator_payload(&raw).context("parse curator payload")?;
    if payload.updates.is_empty() {
        info!(user_id=%user_id, message_id=message_id, "memory ingest produced no updates");
        return Ok(());
    }

    let filtered_updates: Vec<&CuratorUpdate> = payload
        .updates
        .iter()
        .filter(|u| !u.statement.trim().is_empty())
        .collect();
    if filtered_updates.is_empty() {
        return Ok(());
    }

    let update_texts: Vec<String> = filtered_updates
        .iter()
        .map(|u| u.statement.trim().to_string())
        .collect();
    let statement_embeddings =
        embed_many_texts(&embed_backend.client, &embed_backend.model, &update_texts)
            .await
            .context("embed statements")?;
    let mut text_to_statement_id: HashMap<String, i64> = HashMap::new();

    for (upd, emb) in filtered_updates.iter().zip(statement_embeddings.iter()) {
        let statement_text = upd.statement.trim();
        if statement_text.is_empty() || emb.is_empty() {
            continue;
        }

        let mut match_id = 0_i64;
        if let Some((rec, distance)) = uds
            .nearest_memory_statements(&embed_backend.model, emb, 1, Some(target_conversation))
            .ok()
            .and_then(|mut v| v.pop())
        {
            let sim = (1.0 - distance).clamp(-1.0, 1.0).max(0.0);
            if sim >= 0.88 {
                match_id = rec.id;
            }
        }

        let confidence = if upd.confidence <= 0.0 {
            0.6
        } else {
            upd.confidence
        }
        .clamp(0.0, 1.0);
        let salience = if upd.salience <= 0.0 {
            0.5
        } else {
            upd.salience
        }
        .clamp(0.0, 1.0);
        let delta = (upd.delta * confidence).clamp(-3.0, 3.0);

        let sid = uds.apply_memory_patch(db::MemoryPatch {
            statement_id: match_id,
            model: &embed_backend.model,
            text: statement_text,
            delta,
            salience,
            confidence,
            message_id,
            note: upd.evidence.trim(),
            embedding: emb,
        })?;
        text_to_statement_id.insert(statement_key(statement_text), sid);
    }

    for link in payload.links {
        let Some(from_id) = text_to_statement_id
            .get(&statement_key(&link.from))
            .copied()
        else {
            continue;
        };
        let Some(to_id) = text_to_statement_id.get(&statement_key(&link.to)).copied() else {
            continue;
        };
        let kind = normalize_link_kind(&link.kind);
        uds.upsert_memory_link(from_id, to_id, kind, link.strength)?;
    }

    info!(
        user_id = %user_id,
        message_id = message_id,
        updates = text_to_statement_id.len(),
        "memory ingest applied"
    );

    Ok(())
}

async fn embed_single_text(
    client: &openrouter::Client,
    model: &str,
    text: &str,
) -> anyhow::Result<Vec<f32>> {
    let req = openrouter::EmbeddingRequest::single(model.to_string(), text.to_string());
    let resp = client.create_embedding(&req).await?;
    let first = resp
        .data
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing embedding"))?;
    if first.embedding.is_empty() {
        return Err(anyhow::anyhow!("empty embedding"));
    }
    Ok(first.embedding.clone())
}

async fn embed_many_texts(
    client: &openrouter::Client,
    model: &str,
    texts: &[String],
) -> anyhow::Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let req = openrouter::EmbeddingRequest {
        model: model.to_string(),
        input: openrouter::EmbeddingInput::Strings(texts.to_vec()),
    };
    let mut resp = client.create_embedding(&req).await?;
    if resp.data.is_empty() {
        return Err(anyhow::anyhow!("embedding response is empty"));
    }
    resp.data.sort_by(|a, b| a.index.cmp(&b.index));
    let out = resp
        .data
        .into_iter()
        .map(|d| d.embedding)
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    if out.len() != texts.len() {
        return Err(anyhow::anyhow!(
            "embedding count mismatch: got {}, expected {}",
            out.len(),
            texts.len()
        ));
    }
    Ok(out)
}

fn assign_semantic_threads(
    data_dir: &PathBuf,
    runtime: &mut SemanticThreadRuntime,
    uds: &db::UserDataStore,
    user_id: &str,
    model: &str,
    message_id: i64,
    embedding: &[f32],
) -> anyhow::Result<()> {
    if embedding.is_empty() || message_id <= 0 {
        return Ok(());
    }

    let nearest = {
        let cache = runtime.ensure_loaded(data_dir, uds, user_id, model, embedding.len())?;
        cache.nearest(
            embedding,
            SEMANTIC_THREAD_NEAREST_K,
            SEMANTIC_THREAD_SECONDARY_THRESHOLD,
        )
    };

    let mut memberships: Vec<(i64, f64, bool)> = Vec::new();
    if let Some((primary_id, primary_score)) = nearest.first().copied() {
        if primary_score >= SEMANTIC_THREAD_PRIMARY_THRESHOLD {
            memberships.push((primary_id, primary_score, true));
            for (tid, score) in nearest.iter().skip(1).copied() {
                if memberships.len().saturating_sub(1) >= SEMANTIC_THREAD_MAX_SECONDARY {
                    break;
                }
                if score < SEMANTIC_THREAD_SECONDARY_THRESHOLD || tid == primary_id {
                    continue;
                }
                memberships.push((tid, score, false));
            }
        }
    }

    let mut seeded_anchor: Option<(i64, f64, Vec<f32>)> = None;
    if memberships.is_empty() {
        if let Some((anchor, anchor_score)) = uds
            .nearest_semantic_pending_anchor(model, embedding, SEMANTIC_THREAD_PRIMARY_THRESHOLD)
            .context("nearest semantic pending anchor")?
        {
            if anchor.message_id != message_id {
                let tid = uds
                    .create_semantic_thread(model, embedding)
                    .context("create semantic thread")?;
                {
                    let cache = runtime.ensure_loaded(data_dir, uds, user_id, model, embedding.len())?;
                    cache.add_thread(tid, embedding);
                }
                let merged_tid = merge_new_semantic_thread_if_needed(
                    data_dir,
                    runtime,
                    uds,
                    user_id,
                    model,
                    embedding,
                    tid,
                )?;
                memberships.push((merged_tid, 1.0, true));
                seeded_anchor = Some((anchor.message_id, anchor_score, anchor.embedding));
            }
        }
    }

    if memberships.is_empty() {
        uds.upsert_semantic_pending_anchor(message_id, model, embedding)
            .context("upsert semantic pending anchor")?;
        runtime.flush_current(data_dir, false)?;
        return Ok(());
    }

    uds.remove_semantic_pending_anchor(message_id)
        .context("remove current message pending anchor")?;

    if let Some((anchor_message_id, anchor_score, anchor_embedding)) = seeded_anchor {
        let (tid, _, _) = memberships[0];
        let inserted = uds
            .upsert_semantic_thread_membership(tid, anchor_message_id, anchor_score, true)
            .context("upsert seeded anchor membership")?;
        if inserted {
            let cache = runtime.ensure_loaded(data_dir, uds, user_id, model, embedding.len())?;
            cache.apply_membership_insert(tid, &anchor_embedding);
        }
        uds.remove_semantic_pending_anchor(anchor_message_id)
            .context("remove semantic pending anchor")?;
    }

    for (tid, score, is_primary) in memberships.iter().copied() {
        let inserted = uds
            .upsert_semantic_thread_membership(tid, message_id, score, is_primary)
            .context("upsert semantic membership")?;
        if inserted && is_primary {
            let cache = runtime.ensure_loaded(data_dir, uds, user_id, model, embedding.len())?;
            cache.apply_membership_insert(tid, embedding);
        }
    }

    if memberships.len() > 1 {
        for i in 0..memberships.len() {
            for j in (i + 1)..memberships.len() {
                let (a, sa, _) = memberships[i];
                let (b, sb, _) = memberships[j];
                let strength = sa.min(sb);
                uds.upsert_semantic_thread_edge(a, b, "related", strength)
                    .context("upsert semantic thread edge")?;
            }
        }
    }

    runtime.flush_current(data_dir, false)?;
    Ok(())
}

fn merge_new_semantic_thread_if_needed(
    data_dir: &PathBuf,
    runtime: &mut SemanticThreadRuntime,
    uds: &db::UserDataStore,
    user_id: &str,
    model: &str,
    embedding: &[f32],
    new_thread_id: i64,
) -> anyhow::Result<i64> {
    if new_thread_id <= 0 || embedding.is_empty() {
        return Ok(new_thread_id);
    }

    let mut canonical_thread_id = new_thread_id;
    for _ in 0..8 {
        let candidate = {
            let cache = runtime.ensure_loaded(data_dir, uds, user_id, model, embedding.len())?;
            cache
                .nearest(
                    embedding,
                    SEMANTIC_THREAD_NEAREST_K,
                    SEMANTIC_THREAD_MERGE_THRESHOLD,
                )
                .into_iter()
                .find(|(tid, _)| *tid != canonical_thread_id)
        };
        let Some((target_thread_id, _score)) = candidate else {
            break;
        };

        runtime.flush_current(data_dir, true)?;
        let merged = uds
            .merge_semantic_threads(target_thread_id, canonical_thread_id)
            .context("merge semantic threads")?;
        if !merged {
            break;
        }
        canonical_thread_id = target_thread_id;
        runtime.invalidate_current();
    }

    Ok(canonical_thread_id)
}

fn parse_curator_payload(raw: &str) -> anyhow::Result<CuratorPayload> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(CuratorPayload::default());
    }
    if let Ok(v) = serde_json::from_str::<CuratorPayload>(t) {
        return Ok(v);
    }

    let start = t
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("json object not found"))?;
    let end = t
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("json object not found"))?;
    if end <= start {
        return Err(anyhow::anyhow!("invalid json bounds"));
    }
    let candidate = &t[start..=end];
    Ok(serde_json::from_str::<CuratorPayload>(candidate)?)
}

fn statement_key(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn parse_conversation_id(raw: &str) -> Option<i64> {
    let c = raw.trim();
    if c.is_empty() {
        return None;
    }
    let n = c.parse::<i64>().ok()?;
    if n <= 0 {
        return None;
    }
    Some(n)
}

fn normalize_link_kind(kind: &str) -> &str {
    match kind.trim().to_ascii_lowercase().as_str() {
        "supports" => "supports",
        "conflicts" => "conflicts",
        _ => "related",
    }
}

fn message_for_ingest(
    uds: &db::UserDataStore,
    message_id: i64,
) -> anyhow::Result<Option<db::ChatMessageRecord>> {
    match uds.message_by_id(message_id) {
        Ok(m) => {
            if m.role != "user" || m.content.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(m))
            }
        }
        Err(db::DbError::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn sanitize_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

fn belief_strength(score: f64) -> f64 {
    // Smoothly compress to [0, 1] so large repeated evidence does not dominate relevance.
    ((score.tanh()) + 1.0) * 0.5
}

fn normalize_unit_vector(v: &[f32]) -> Vec<f32> {
    if v.is_empty() {
        return Vec::new();
    }
    let mut norm_sq = 0.0_f64;
    for x in v {
        let xf = f64::from(*x);
        norm_sq += xf * xf;
    }
    if norm_sq <= 0.0 || !norm_sq.is_finite() {
        return Vec::new();
    }
    let inv_norm = (1.0 / norm_sq.sqrt()) as f32;
    v.iter().map(|x| *x * inv_norm).collect()
}

fn project_and_normalize(v: &[f32], out_dims: usize) -> Vec<f32> {
    if v.is_empty() || out_dims == 0 {
        return Vec::new();
    }
    let mut out = vec![0.0_f32; out_dims];
    for (i, x) in v.iter().enumerate() {
        let h = mix64(i as u64);
        let idx = (h as usize) % out_dims;
        let sign = if ((h >> 63) & 1) == 0 {
            1.0_f32
        } else {
            -1.0_f32
        };
        out[idx] += *x * sign;
    }
    normalize_unit_vector(&out)
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

fn ann_plane_coeff(table: usize, bit: usize, dim: usize) -> f32 {
    let seed = ((table as u64 + 1) << 40)
        ^ ((bit as u64 + 1) << 24)
        ^ (dim as u64 + 1)
        ^ 0x9e37_79b9_7f4a_7c15;
    let h = mix64(seed);
    let v = ((h & 0xffff) as f32 / 65535.0) * 2.0 - 1.0;
    if v == 0.0 {
        1.0e-4
    } else {
        v
    }
}

fn add_ann_votes(votes: &mut HashMap<usize, u16>, bucket: &[usize]) {
    for idx in bucket {
        let entry = votes.entry(*idx).or_insert(0);
        *entry = entry.saturating_add(1);
    }
}

fn remove_ann_index(bucket: &mut Vec<usize>, idx: usize) {
    if let Some(pos) = bucket.iter().position(|v| *v == idx) {
        bucket.swap_remove(pos);
    }
}

fn dot_product_simd(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    #[cfg(target_arch = "aarch64")]
    {
        // Safety: aarch64 guarantees NEON; slices are equal-len and valid.
        unsafe { dot_product_neon(a, b) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                // Safety: AVX2 support is runtime-checked above.
                return unsafe { dot_product_avx2(a, b) };
            }
        }
        dot_product_scalar(a, b)
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    let mut out = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        out += *x * *y;
    }
    out
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn dot_product_neon(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::aarch64::*;

    let len = a.len();
    let mut i = 0usize;
    let mut acc = vdupq_n_f32(0.0);
    while i + 4 <= len {
        let va = unsafe { vld1q_f32(a.as_ptr().add(i)) };
        let vb = unsafe { vld1q_f32(b.as_ptr().add(i)) };
        acc = vmlaq_f32(acc, va, vb);
        i += 4;
    }
    let mut out = vaddvq_f32(acc);
    while i < len {
        out += unsafe { *a.get_unchecked(i) * *b.get_unchecked(i) };
        i += 1;
    }
    out
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dot_product_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = a.len();
    let mut i = 0usize;
    let mut acc = _mm256_setzero_ps();
    while i + 8 <= len {
        let va = unsafe { _mm256_loadu_ps(a.as_ptr().add(i)) };
        let vb = unsafe { _mm256_loadu_ps(b.as_ptr().add(i)) };
        let prod = _mm256_mul_ps(va, vb);
        acc = _mm256_add_ps(acc, prod);
        i += 8;
    }
    let mut tmp = [0.0_f32; 8];
    unsafe { _mm256_storeu_ps(tmp.as_mut_ptr(), acc) };
    let mut out = tmp.into_iter().sum::<f32>();
    while i < len {
        out += unsafe { *a.get_unchecked(i) * *b.get_unchecked(i) };
        i += 1;
    }
    out
}

fn curator_disabled_for_now() -> bool {
    true
}

#[cfg(test)]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut na = 0.0_f64;
    let mut nb = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_curator_payload_accepts_wrapped_json() {
        let raw = "Here is output:\n{\"updates\":[{\"statement\":\"likes beer\",\"delta\":0.6,\"salience\":0.8,\"confidence\":0.7}],\"links\":[]}";
        let p = parse_curator_payload(raw).unwrap();
        assert_eq!(p.updates.len(), 1);
        assert_eq!(p.updates[0].statement, "likes beer");
    }

    #[test]
    fn cosine_similarity_handles_basic_cases() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) > 0.999);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) < 0.001);
    }
}
