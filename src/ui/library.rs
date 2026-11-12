//! The application model: owns the store and every cache derived from it.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::NaiveDate;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};

use crate::api_server::ApiServer;
use crate::core::model::Settings;
use crate::core::semantic::SemanticIndex;
use crate::core::stats::{self, DayStat, Overview};
use crate::core::store::{Store, content_hash};
use crate::core::tags::{self, TagNode};
use crate::core::ai;

use super::tasks::run_blocking;

#[derive(Clone, Debug, PartialEq)]
pub enum EmbeddingStatus {
    Disabled,
    Idle,
    Running { done: usize, total: usize },
    Failed(String),
}

type DenseMap = HashMap<String, (String, Vec<f32>)>;

pub struct Library {
    store: Store,
    index: Arc<SemanticIndex>,
    index_building: bool,
    index_dirty: bool,
    dense: Arc<DenseMap>,
    dense_model: String,
    tag_counts: Rc<BTreeMap<String, usize>>,
    tag_tree: Rc<Vec<TagNode>>,
    overview: Overview,
    day_stats: Rc<HashMap<NaiveDate, DayStat>>,
    embedding_status: EmbeddingStatus,
    api_server: Option<ApiServer>,
    api_error: Option<String>,
    _poll_task: Task<()>,
    _index_task: Option<Task<()>>,
    _embedding_task: Option<Task<()>>,
}

struct LibraryGlobal(Entity<Library>);

impl Global for LibraryGlobal {}

impl Library {
    pub fn init(store: Store, cx: &mut App) -> Entity<Library> {
        let library = cx.new(|cx| Library::new(store, cx));
        cx.set_global(LibraryGlobal(library.clone()));
        library
    }

    pub fn global(cx: &App) -> Entity<Library> {
        cx.global::<LibraryGlobal>().0.clone()
    }

    fn new(store: Store, cx: &mut Context<Self>) -> Self {
        let poll_task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let Ok(()) = this.update(cx, |this, cx| {
                    let before = this.store.settings().clone();
                    if let Ok(true) = this.store.sync_external_changes() {
                        this.apply_settings_change(&before, cx);
                        this.changed(cx);
                    }
                }) else {
                    break;
                };
            }
        });
        let mut library = Self {
            store,
            index: Arc::new(SemanticIndex::empty()),
            index_building: false,
            index_dirty: false,
            dense: Arc::new(HashMap::new()),
            dense_model: String::new(),
            tag_counts: Rc::new(BTreeMap::new()),
            tag_tree: Rc::new(Vec::new()),
            overview: Overview::default(),
            day_stats: Rc::new(HashMap::new()),
            embedding_status: EmbeddingStatus::Disabled,
            api_server: None,
            api_error: None,
            _poll_task: poll_task,
            _index_task: None,
            _embedding_task: None,
        };
        library.load_dense();
        library.sync_api_server();
        library.changed(cx);
        library
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn settings(&self) -> &Settings {
        self.store.settings()
    }

    pub fn index(&self) -> Arc<SemanticIndex> {
        self.index.clone()
    }

    pub fn is_indexing(&self) -> bool {
        self.index_building
    }

    pub fn tag_counts(&self) -> Rc<BTreeMap<String, usize>> {
        self.tag_counts.clone()
    }

    pub fn tag_tree(&self) -> Rc<Vec<TagNode>> {
        self.tag_tree.clone()
    }

    pub fn overview(&self) -> &Overview {
        &self.overview
    }

    pub fn day_stats(&self) -> Rc<HashMap<NaiveDate, DayStat>> {
        self.day_stats.clone()
    }

    pub fn embedding_status(&self) -> &EmbeddingStatus {
        &self.embedding_status
    }

    pub fn api_error(&self) -> Option<&str> {
        self.api_error.as_deref()
    }

    pub fn api_running(&self) -> bool {
        self.api_server.is_some()
    }

    pub fn ref_label(&self, id: &str) -> String {
        match self.store.get(id) {
            Some(m) if !m.is_deleted() => m.snippet(16),
            Some(_) => t!("已删除的笔记").into(),
            None => t!("笔记不存在").into(),
        }
    }

    /// Picks up writes made by other processes (MCP, API, a second window) right away
    /// instead of waiting for the next poll. Returns whether anything changed.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) -> Result<bool> {
        let before = self.store.settings().clone();
        let changed = self.store.sync_external_changes()?;
        if changed {
            self.apply_settings_change(&before, cx);
        }
        self.changed(cx);
        Ok(changed)
    }

    /// For writes that do not touch memos (insights, chat, memory profile).
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Runs a store mutation and refreshes every derived cache.
    pub fn mutate<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Store) -> Result<R>,
    ) -> Result<R> {
        let result = f(&mut self.store);
        self.changed(cx);
        result
    }

    pub fn update_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) -> Result<()> {
        let before = self.store.settings().clone();
        self.store.update_settings(f)?;
        self.apply_settings_change(&before, cx);
        cx.notify();
        Ok(())
    }

    fn apply_settings_change(&mut self, before: &Settings, cx: &mut Context<Self>) {
        let after = self.store.settings().clone();
        if before.ai.embedding_model != after.ai.embedding_model || before.ai.base_url != after.ai.base_url {
            self._embedding_task = None;
            self.load_dense();
            self.schedule_index(cx);
        }
        if before.language != after.language {
            crate::i18n::set_language(after.language);
        }
        if before.api_enabled != after.api_enabled
            || before.api_port != after.api_port
            || before.api_token != after.api_token
        {
            self.sync_api_server();
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let counts = tags::tag_counts(self.store.active_memos());
        self.tag_tree = Rc::new(tags::build_tag_tree(&counts));
        self.tag_counts = Rc::new(counts);
        self.overview = stats::overview(self.store.active_memos());
        self.day_stats = Rc::new(stats::per_day(self.store.active_memos()));
        self.schedule_index(cx);
        cx.notify();
    }

    fn load_dense(&mut self) {
        let cfg = &self.store.settings().ai;
        if cfg.uses_remote_embeddings() {
            self.dense_model = cfg.embedding_model.trim().to_string();
            self.dense = Arc::new(self.store.load_embeddings(&self.dense_model).unwrap_or_default());
            self.embedding_status = EmbeddingStatus::Idle;
        } else {
            self.dense_model.clear();
            self.dense = Arc::new(HashMap::new());
            self.embedding_status = EmbeddingStatus::Disabled;
        }
    }

    fn schedule_index(&mut self, cx: &mut Context<Self>) {
        if self.index_building {
            self.index_dirty = true;
            return;
        }
        self.index_building = true;
        self.index_dirty = false;
        let docs: Vec<(String, String)> = self
            .store
            .active_memos()
            .map(|m| (m.id.clone(), m.content.clone()))
            .collect();
        let dense = self.dense.clone();
        self._index_task = Some(cx.spawn(async move |this, cx| {
            let index = cx
                .background_spawn(async move {
                    let vectors: HashMap<String, Vec<f32>> = docs
                        .iter()
                        .filter_map(|(id, content)| {
                            let (hash, v) = dense.get(id)?;
                            (*hash == content_hash(content)).then(|| (id.clone(), v.clone()))
                        })
                        .collect();
                    SemanticIndex::build(&docs, vectors)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.index = Arc::new(index);
                this.index_building = false;
                if this.index_dirty {
                    this.schedule_index(cx);
                } else {
                    this.start_embedding_sync(cx);
                }
                cx.notify();
            });
        }));
    }

    pub fn resync_embeddings(&mut self, cx: &mut Context<Self>) {
        if matches!(self.embedding_status, EmbeddingStatus::Failed(_)) {
            self.embedding_status = EmbeddingStatus::Idle;
        }
        self.start_embedding_sync(cx);
    }

    fn start_embedding_sync(&mut self, cx: &mut Context<Self>) {
        let cfg = self.store.settings().ai.clone();
        if !cfg.uses_remote_embeddings()
            || self.dense_model.is_empty()
            || matches!(
                self.embedding_status,
                EmbeddingStatus::Running { .. } | EmbeddingStatus::Failed(_)
            )
        {
            return;
        }
        let pending: Vec<(String, String, String)> = self
            .store
            .active_memos()
            .filter_map(|m| {
                let hash = content_hash(&m.content);
                let fresh = self.dense.get(&m.id).is_some_and(|(h, _)| *h == hash);
                (!fresh).then(|| {
                    let text: String = crate::core::parse::plain_text(&m.content).chars().take(2000).collect();
                    (m.id.clone(), hash, if text.is_empty() { " ".into() } else { text })
                })
            })
            .collect();
        if pending.is_empty() {
            self.embedding_status = EmbeddingStatus::Idle;
            return;
        }
        let total = pending.len();
        self.embedding_status = EmbeddingStatus::Running { done: 0, total };
        let model = self.dense_model.clone();
        self._embedding_task = Some(cx.spawn(async move |this, cx| {
            let mut done = 0;
            for chunk in pending.chunks(16) {
                let chunk = chunk.to_vec();
                let inputs: Vec<String> = chunk.iter().map(|(_, _, t)| t.clone()).collect();
                let cfg = cfg.clone();
                let result = run_blocking(move || ai::embed(&cfg, &inputs)).await;
                let outcome = this.update(cx, |this, cx| match result {
                    Some(Ok(vectors)) => {
                        let items: Vec<(String, String, Vec<f32>)> = chunk
                            .into_iter()
                            .zip(vectors)
                            .map(|((id, hash, _), v)| (id, hash, v))
                            .collect();
                        let _ = this.store.save_embeddings(&model, &items);
                        let dense = Arc::make_mut(&mut this.dense);
                        for (id, hash, v) in items {
                            dense.insert(id, (hash, v));
                        }
                        done += 16.min(total - done);
                        this.embedding_status = EmbeddingStatus::Running { done, total };
                        cx.notify();
                        true
                    }
                    Some(Err(err)) => {
                        this.embedding_status = EmbeddingStatus::Failed(err.to_string());
                        cx.notify();
                        false
                    }
                    None => false,
                });
                if !matches!(outcome, Ok(true)) {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.embedding_status = EmbeddingStatus::Idle;
                this.schedule_index(cx);
            });
        }));
    }

    fn sync_api_server(&mut self) {
        let settings = self.store.settings().clone();
        self.api_server = None;
        self.api_error = None;
        if !settings.api_enabled {
            return;
        }
        match ApiServer::start(
            self.store.data_dir().to_path_buf(),
            settings.api_port,
            settings.api_token.clone(),
        ) {
            Ok(server) => self.api_server = Some(server),
            Err(err) => self.api_error = Some(err.to_string()),
        }
    }
}
