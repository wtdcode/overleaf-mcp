use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use overleaf_client::OverleafClient;
use overleaf_types::{
    AppliedOtUpdate, EntityKind, EntityRefJson, JoinProjectArgs, OtComponent, OtUpdate,
    OverleafError, ProjectTree, RealtimeSettings, Result,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, oneshot, watch};

use crate::frame::{EventPayload, Frame};

#[derive(Serialize)]
struct JoinDocOptions {
    #[serde(rename = "encodeRanges")]
    encode_ranges: bool,
}

#[derive(Debug, Clone)]
enum Phase {
    Connecting,
    Ready,
    Dead(String),
}

#[derive(Debug)]
pub struct EditOutcome {
    pub content: String,
    pub version: i64,
    pub changed: bool,
}

#[derive(Debug)]
struct InFlight {
    tx: oneshot::Sender<std::result::Result<i64, String>>,
}

/// In-memory mirror of one joined document. `content` tracks the server state
/// as long as `dirty` is false; any divergence we cannot reconstruct locally
/// (transformed ops, apply failures, reconnects) just flips `dirty` and the
/// next reader re-joins the doc for a fresh authoritative copy.
#[derive(Debug)]
pub struct DocShadow {
    pub content: String,
    pub version: i64,
    dirty: bool,
    raced: bool,
    in_flight: Option<InFlight>,
}

impl DocShadow {
    /// joinDoc lines arrive as UTF-8 bytes smuggled through code points 0-255
    /// (`unescape(encodeURIComponent(line))` server-side); ASCII passes through
    /// unchanged. Anything that does not look like that encoding is kept as-is.
    fn decode_wire_line(raw: &str) -> String {
        if raw.is_ascii() {
            return raw.to_string();
        }
        let mut bytes = Vec::with_capacity(raw.len());
        for ch in raw.chars() {
            let cp = ch as u32;
            if cp > 0xFF {
                tracing::warn!("unexpected non-byte code point in wire line");
                return raw.to_string();
            }
            bytes.push(cp as u8);
        }
        match String::from_utf8(bytes) {
            Ok(decoded) => decoded,
            Err(_) => raw.to_string(),
        }
    }

    fn apply_component(&mut self, comp: &OtComponent) -> Result<()> {
        if comp.c.is_some() {
            return Ok(());
        }
        if let Some(text) = &comp.i {
            let at = OtComponent::byte_of_utf16(&self.content, comp.p).ok_or_else(|| {
                OverleafError::OutOfSync(format!("insert position {} out of range", comp.p))
            })?;
            self.content.insert_str(at, text);
        } else if let Some(text) = &comp.d {
            let start = OtComponent::byte_of_utf16(&self.content, comp.p).ok_or_else(|| {
                OverleafError::OutOfSync(format!("delete position {} out of range", comp.p))
            })?;
            let end = OtComponent::byte_of_utf16(
                &self.content,
                comp.p + OtComponent::utf16_len(text),
            )
            .ok_or_else(|| {
                OverleafError::OutOfSync(format!("delete end for position {} out of range", comp.p))
            })?;
            if self.content.get(start..end) != Some(text.as_str()) {
                return Err(OverleafError::OutOfSync(
                    "deleted text does not match shadow content".to_string(),
                ));
            }
            self.content.replace_range(start..end, "");
        }
        Ok(())
    }

    fn fail_in_flight(&mut self, reason: &str) {
        if let Some(inflight) = self.in_flight.take() {
            let _ = inflight.tx.send(Err(reason.to_string()));
        }
    }
}

struct ConnState {
    public_id: Option<String>,
    tree: Option<ProjectTree>,
    tree_dirty: bool,
    docs: BTreeMap<String, DocShadow>,
}

struct ConnInner {
    client: Arc<OverleafClient>,
    project_id: String,
    sid: String,
    settings: RealtimeSettings,
    next_msg_id: AtomicU64,
    stop: AtomicBool,
    acks: Mutex<BTreeMap<u64, oneshot::Sender<Vec<Value>>>>,
    state: Mutex<ConnState>,
    phase_tx: watch::Sender<Phase>,
    phase_rx: watch::Receiver<Phase>,
    edit_locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    /// real-time rejects parallel joinDoc calls on one socket with a
    /// `joinLeaveEpoch mismatch`, so joins are serialized per connection.
    join_lock: Mutex<()>,
}

impl ConnInner {
    fn op_timeout(&self) -> Duration {
        Duration::from_secs(self.settings.op_timeout_secs)
    }

    async fn die(&self, reason: String) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!(project = %self.project_id, "realtime connection closed: {reason}");
        let _ = self.phase_tx.send(Phase::Dead(reason.clone()));
        self.acks.lock().await.clear();
        let mut st = self.state.lock().await;
        for shadow in st.docs.values_mut() {
            shadow.dirty = true;
            shadow.fail_in_flight(&reason);
        }
    }

    async fn poll_loop(inner: Arc<ConnInner>) {
        let timeout = Duration::from_secs(inner.settings.poll_timeout_secs);
        while !inner.stop.load(Ordering::SeqCst) {
            let raw = match inner.client.rt_poll(&inner.sid, timeout).await {
                Ok(raw) => raw,
                Err(err) => {
                    inner.die(format!("poll failed: {err}")).await;
                    break;
                }
            };
            for piece in Frame::split_batch(&raw) {
                let frame = match Frame::parse(&piece) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!("unparseable frame: {err}");
                        continue;
                    }
                };
                inner.dispatch(frame).await;
                if inner.stop.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }

    async fn dispatch(&self, frame: Frame) {
        match frame {
            Frame::Heartbeat => {
                if let Err(err) = self.client.rt_send(&self.sid, "2::".to_string()).await {
                    self.die(format!("heartbeat send failed: {err}")).await;
                }
            }
            Frame::Ack { id, args } => {
                if let Some(tx) = self.acks.lock().await.remove(&id) {
                    let _ = tx.send(args);
                }
            }
            Frame::Event(payload) => self.handle_event(payload).await,
            Frame::ProtoError { reason } => {
                self.die(format!("socket.io error frame: {reason}")).await;
            }
            Frame::Disconnect => {
                self.die("server sent disconnect".to_string()).await;
            }
            Frame::Connect | Frame::Noop | Frame::Message(_) | Frame::JsonMsg(_) => {}
        }
    }

    async fn handle_event(&self, payload: EventPayload) {
        let name = payload.name.as_str();
        match name {
            "joinProjectResponse" => {
                let parsed: std::result::Result<JoinProjectArgs, _> = payload
                    .args
                    .first()
                    .cloned()
                    .map(serde_json::from_value)
                    .unwrap_or_else(|| Err(serde::de::Error::custom("missing args")));
                match parsed.and_then(|args| {
                    ProjectTree::from_join(&args.project)
                        .map(|tree| (args, tree))
                        .map_err(|e| serde::de::Error::custom(e.to_string()))
                }) {
                    Ok((args, tree)) => {
                        {
                            let mut st = self.state.lock().await;
                            st.public_id = args.public_id;
                            st.tree = Some(tree);
                            st.tree_dirty = false;
                        }
                        let _ = self.phase_tx.send(Phase::Ready);
                    }
                    Err(err) => {
                        self.die(format!("bad joinProjectResponse: {err}")).await;
                    }
                }
            }
            "connectionRejected" => {
                let detail = payload
                    .args
                    .first()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "no detail".to_string());
                self.die(format!("connection rejected: {detail}")).await;
            }
            "otUpdateApplied" => {
                let update: AppliedOtUpdate = match payload
                    .args
                    .first()
                    .cloned()
                    .map(serde_json::from_value)
                {
                    Some(Ok(update)) => update,
                    _ => {
                        tracing::warn!("unparseable otUpdateApplied event");
                        return;
                    }
                };
                self.on_update_applied(update).await;
            }
            "otUpdateError" => {
                let detail = payload
                    .args
                    .first()
                    .map(|v| {
                        v.get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                let doc_id = payload
                    .args
                    .get(1)
                    .and_then(|v| v.get("doc"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let mut st = self.state.lock().await;
                match doc_id.and_then(|id| st.docs.get_mut(&id)) {
                    Some(shadow) => {
                        shadow.dirty = true;
                        shadow.fail_in_flight(&detail);
                    }
                    None => {
                        for shadow in st.docs.values_mut() {
                            shadow.dirty = true;
                            shadow.fail_in_flight(&detail);
                        }
                    }
                }
            }
            "reciveNewDoc" | "reciveNewFile" | "reciveNewFolder" => {
                let kind = match name {
                    "reciveNewDoc" => EntityKind::Doc,
                    "reciveNewFile" => EntityKind::File,
                    _ => EntityKind::Folder,
                };
                let folder_id = payload.args.first().and_then(Value::as_str);
                let entity: Option<EntityRefJson> = payload
                    .args
                    .get(1)
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok());
                let mut st = self.state.lock().await;
                if let (Some(folder_id), Some(entity), Some(tree)) =
                    (folder_id, entity, st.tree.as_mut())
                {
                    tree.insert_node(entity.id, entity.name, kind, folder_id);
                } else {
                    st.tree_dirty = true;
                }
            }
            "removeEntity" => {
                let entity_id = payload.args.first().and_then(Value::as_str);
                let mut st = self.state.lock().await;
                match entity_id {
                    Some(id) => {
                        let removed = match st.tree.as_mut() {
                            Some(tree) => tree.remove_node(id),
                            None => Vec::new(),
                        };
                        for gone in removed {
                            if let Some(mut shadow) = st.docs.remove(&gone) {
                                shadow.fail_in_flight("document was deleted");
                            }
                        }
                    }
                    None => st.tree_dirty = true,
                }
            }
            "reciveEntityRename" => {
                let entity_id = payload.args.first().and_then(Value::as_str);
                let new_name = payload.args.get(1).and_then(Value::as_str);
                let mut st = self.state.lock().await;
                let applied = match (entity_id, new_name, st.tree.as_mut()) {
                    (Some(id), Some(name), Some(tree)) => tree.rename_node(id, name.to_string()),
                    _ => false,
                };
                if !applied {
                    st.tree_dirty = true;
                }
            }
            "reciveEntityMove" => {
                let entity_id = payload.args.first().and_then(Value::as_str);
                let folder_id = payload.args.get(1).and_then(Value::as_str);
                let mut st = self.state.lock().await;
                let applied = match (entity_id, folder_id, st.tree.as_mut()) {
                    (Some(id), Some(folder), Some(tree)) => tree.reparent_node(id, folder),
                    _ => false,
                };
                if !applied {
                    st.tree_dirty = true;
                }
            }
            _ => {
                tracing::trace!("ignoring realtime event {name}");
            }
        }
    }

    async fn on_update_applied(&self, update: AppliedOtUpdate) {
        let mut st = self.state.lock().await;
        let Some(shadow) = st.docs.get_mut(&update.doc) else {
            return;
        };
        match update.op {
            // Short confirmation of our own in-flight op.
            None => {
                if let Some(inflight) = shadow.in_flight.take() {
                    let _ = inflight.tx.send(Ok(update.v));
                }
            }
            // A remote client's op. While we have an op in flight the server
            // may interleave and transform, which we cannot replay locally, so
            // the shadow is resynced after the in-flight op settles.
            Some(components) => {
                if shadow.in_flight.is_some() {
                    shadow.raced = true;
                    shadow.dirty = true;
                } else if update.v == shadow.version {
                    let mut ok = true;
                    for comp in &components {
                        if let Err(err) = shadow.apply_component(comp) {
                            tracing::warn!(doc = %update.doc, "remote op apply failed: {err}");
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        shadow.version = update.v + 1;
                    } else {
                        shadow.dirty = true;
                    }
                } else {
                    tracing::debug!(
                        doc = %update.doc,
                        "remote op at version {} but shadow at {}",
                        update.v,
                        shadow.version
                    );
                    shadow.dirty = true;
                }
            }
        }
    }

    async fn emit_with_ack(&self, name: &str, args: Vec<Value>) -> Result<Vec<Value>> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(OverleafError::Protocol(
                "realtime connection is closed".to_string(),
            ));
        }
        let id = self.next_msg_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.acks.lock().await.insert(id, tx);
        let payload = EventPayload {
            name: name.to_string(),
            args,
        };
        let encoded = Frame::encode_event(&payload, Some(id))?;
        if let Err(err) = self.client.rt_send(&self.sid, encoded).await {
            self.acks.lock().await.remove(&id);
            return Err(err);
        }
        match tokio::time::timeout(self.op_timeout(), rx).await {
            Ok(Ok(args)) => Ok(args),
            Ok(Err(_)) => Err(OverleafError::Protocol(
                "realtime connection lost while waiting for ack".to_string(),
            )),
            Err(_) => {
                self.acks.lock().await.remove(&id);
                Err(OverleafError::Timeout(format!("no ack for {name}")))
            }
        }
    }
}

/// A live realtime session for one project. Cheap to clone; the polling task
/// runs until `shutdown` or a transport error, after which the connection is
/// permanently dead and a caller should open a fresh one.
#[derive(Clone)]
pub struct ProjectConnection {
    inner: Arc<ConnInner>,
}

impl ProjectConnection {
    pub async fn open(
        client: Arc<OverleafClient>,
        project_id: &str,
        settings: RealtimeSettings,
    ) -> Result<Self> {
        let sid = client.rt_handshake(project_id).await?;
        let (phase_tx, phase_rx) = watch::channel(Phase::Connecting);
        let inner = Arc::new(ConnInner {
            client,
            project_id: project_id.to_string(),
            sid,
            settings,
            next_msg_id: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            acks: Mutex::new(BTreeMap::new()),
            state: Mutex::new(ConnState {
                public_id: None,
                tree: None,
                tree_dirty: false,
                docs: BTreeMap::new(),
            }),
            phase_tx,
            phase_rx,
            edit_locks: Mutex::new(BTreeMap::new()),
            join_lock: Mutex::new(()),
        });
        tokio::spawn(ConnInner::poll_loop(inner.clone()));
        let conn = ProjectConnection { inner };
        conn.wait_ready().await?;
        Ok(conn)
    }

    async fn wait_ready(&self) -> Result<()> {
        let mut rx = self.inner.phase_rx.clone();
        let deadline = Duration::from_secs(self.inner.settings.connect_timeout_secs);
        let waited = tokio::time::timeout(deadline, async move {
            loop {
                let phase = rx.borrow_and_update().clone();
                match phase {
                    Phase::Ready => return Ok(()),
                    Phase::Dead(reason) => return Err(OverleafError::Protocol(reason)),
                    Phase::Connecting => {}
                }
                if rx.changed().await.is_err() {
                    return Err(OverleafError::Protocol(
                        "realtime connection task exited".to_string(),
                    ));
                }
            }
        })
        .await;
        match waited {
            Ok(result) => result,
            Err(_) => Err(OverleafError::Timeout(
                "waiting for joinProjectResponse".to_string(),
            )),
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.inner.stop.load(Ordering::SeqCst)
            && matches!(*self.inner.phase_rx.borrow(), Phase::Ready)
    }

    pub async fn shutdown(&self) {
        let _ = self
            .inner
            .client
            .rt_send(&self.inner.sid, "0::".to_string())
            .await;
        self.inner.die("closed by client".to_string()).await;
    }

    /// Current project tree. Fails with `OutOfSync` when an untrackable tree
    /// change happened; the caller should discard this connection and reopen.
    pub async fn tree(&self) -> Result<ProjectTree> {
        self.wait_ready().await?;
        let st = self.inner.state.lock().await;
        if st.tree_dirty {
            return Err(OverleafError::OutOfSync(
                "project tree changed in a way that could not be tracked".to_string(),
            ));
        }
        st.tree.clone().ok_or_else(|| {
            OverleafError::Protocol("project tree not available".to_string())
        })
    }

    // Local tree fixups mirroring the effects of our own HTTP mutations, so a
    // follow-up call sees them even before the realtime event arrives.
    pub async fn tree_apply_created(
        &self,
        parent_folder_id: &str,
        entity_id: String,
        name: String,
        kind: EntityKind,
    ) {
        let mut st = self.inner.state.lock().await;
        if let Some(tree) = st.tree.as_mut() {
            tree.insert_node(entity_id, name, kind, parent_folder_id);
        }
    }

    pub async fn tree_apply_removed(&self, entity_id: &str) {
        let mut st = self.inner.state.lock().await;
        let removed = match st.tree.as_mut() {
            Some(tree) => tree.remove_node(entity_id),
            None => Vec::new(),
        };
        for gone in removed {
            if let Some(mut shadow) = st.docs.remove(&gone) {
                shadow.fail_in_flight("document was deleted");
            }
        }
    }

    pub async fn tree_apply_renamed(&self, entity_id: &str, name: String) {
        let mut st = self.inner.state.lock().await;
        if let Some(tree) = st.tree.as_mut() {
            tree.rename_node(entity_id, name);
        }
    }

    pub async fn tree_apply_moved(&self, entity_id: &str, folder_id: &str) {
        let mut st = self.inner.state.lock().await;
        if let Some(tree) = st.tree.as_mut() {
            tree.reparent_node(entity_id, folder_id);
        }
    }

    /// Returns the current content and version, joining the doc on first use
    /// and re-joining whenever the shadow was marked dirty.
    pub async fn read_doc(&self, doc_id: &str) -> Result<(String, i64)> {
        {
            let st = self.inner.state.lock().await;
            if let Some(shadow) = st.docs.get(doc_id) {
                // While an op is in flight the shadow is mid-transition; serve
                // the pre-op state instead of clobbering the edit bookkeeping.
                if !shadow.dirty || shadow.in_flight.is_some() {
                    return Ok((shadow.content.clone(), shadow.version));
                }
            }
        }
        self.join_doc(doc_id).await
    }

    async fn join_doc(&self, doc_id: &str) -> Result<(String, i64)> {
        self.wait_ready().await?;
        let _join_guard = self.inner.join_lock.lock().await;
        // A concurrent caller may have completed this join while we waited.
        {
            let st = self.inner.state.lock().await;
            if let Some(shadow) = st.docs.get(doc_id)
                && !shadow.dirty
            {
                return Ok((shadow.content.clone(), shadow.version));
            }
        }
        let options = serde_json::to_value(JoinDocOptions { encode_ranges: true })?;
        let args = self
            .inner
            .emit_with_ack("joinDoc", vec![Value::String(doc_id.to_string()), options])
            .await?;
        if let Some(err) = args.first()
            && !err.is_null() {
                return Err(OverleafError::Protocol(format!(
                    "joinDoc rejected: {err}"
                )));
            }
        let lines = args
            .get(1)
            .and_then(Value::as_array)
            .ok_or_else(|| OverleafError::Protocol("joinDoc returned no lines".to_string()))?;
        let version = args
            .get(2)
            .and_then(Value::as_i64)
            .ok_or_else(|| OverleafError::Protocol("joinDoc returned no version".to_string()))?;
        let ot_type = args
            .get(5)
            .and_then(Value::as_str)
            .unwrap_or("sharejs-text-ot");
        if ot_type != "sharejs-text-ot" {
            return Err(OverleafError::Protocol(format!(
                "doc uses OT type {ot_type}; only sharejs-text-ot is supported"
            )));
        }
        let mut decoded = Vec::with_capacity(lines.len());
        for line in lines {
            let raw = line.as_str().ok_or_else(|| {
                OverleafError::Protocol("joinDoc line is not a string".to_string())
            })?;
            decoded.push(DocShadow::decode_wire_line(raw));
        }
        let content = decoded.join("\n");
        let mut st = self.inner.state.lock().await;
        if let Some(previous) = st.docs.get_mut(doc_id) {
            previous.fail_in_flight("document was re-joined");
        }
        st.docs.insert(
            doc_id.to_string(),
            DocShadow {
                content: content.clone(),
                version,
                dirty: false,
                raced: false,
                in_flight: None,
            },
        );
        Ok((content, version))
    }

    /// Runs one edit transaction: `build` receives the fresh authoritative
    /// content and returns the op to submit (empty means nothing to do). When
    /// the server interleaves a concurrent edit the doc is resynced and `build`
    /// is re-run, up to `edit_retries` times.
    pub async fn edit_doc<F>(&self, doc_id: &str, mut build: F) -> Result<EditOutcome>
    where
        F: FnMut(&str) -> Result<Vec<OtComponent>>,
    {
        let lock = {
            let mut locks = self.inner.edit_locks.lock().await;
            locks
                .entry(doc_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let mut last_err =
            OverleafError::OutOfSync("edit gave up before submitting".to_string());
        for attempt in 0..=self.inner.settings.edit_retries {
            if attempt > 0 {
                tracing::debug!(doc = %doc_id, "retrying edit, attempt {attempt}");
            }
            let (content, version) = self.read_doc(doc_id).await?;
            let ops = build(&content)?;
            if ops.is_empty() {
                return Ok(EditOutcome {
                    content,
                    version,
                    changed: false,
                });
            }
            let (tx, rx) = oneshot::channel();
            {
                let mut st = self.inner.state.lock().await;
                let shadow = st.docs.get_mut(doc_id).ok_or_else(|| {
                    OverleafError::Protocol("doc shadow disappeared during edit".to_string())
                })?;
                shadow.fail_in_flight("superseded by a new edit");
                shadow.raced = false;
                shadow.in_flight = Some(InFlight { tx });
            }
            let update = OtUpdate {
                doc: doc_id.to_string(),
                op: ops.clone(),
                v: version,
            };
            let rpc = self
                .inner
                .emit_with_ack(
                    "applyOtUpdate",
                    vec![
                        Value::String(doc_id.to_string()),
                        serde_json::to_value(&update)?,
                    ],
                )
                .await;
            match rpc {
                Ok(args) => {
                    if let Some(err) = args.first()
                        && !err.is_null() {
                            self.clear_in_flight(doc_id).await;
                            // Validation and size rejections are permanent.
                            return Err(OverleafError::Edit(format!(
                                "server rejected update: {err}"
                            )));
                        }
                }
                Err(err) => {
                    self.clear_in_flight(doc_id).await;
                    last_err = err;
                    continue;
                }
            }
            match tokio::time::timeout(self.inner.op_timeout(), rx).await {
                Err(_) => {
                    self.clear_in_flight(doc_id).await;
                    last_err = OverleafError::Timeout(
                        "op sent but not confirmed; doc will be resynced".to_string(),
                    );
                }
                Ok(Err(_)) => {
                    return Err(OverleafError::Protocol(
                        "realtime connection lost mid-edit".to_string(),
                    ));
                }
                Ok(Ok(Err(reason))) => {
                    last_err = OverleafError::Edit(reason);
                }
                Ok(Ok(Ok(applied_v))) => {
                    let mut st = self.inner.state.lock().await;
                    let shadow = st.docs.get_mut(doc_id).ok_or_else(|| {
                        OverleafError::Protocol("doc shadow disappeared during edit".to_string())
                    })?;
                    if shadow.raced || applied_v != version {
                        shadow.dirty = true;
                        last_err = OverleafError::OutOfSync(
                            "a concurrent edit was interleaved".to_string(),
                        );
                        continue;
                    }
                    let mut applied = true;
                    for comp in &ops {
                        if let Err(err) = shadow.apply_component(comp) {
                            tracing::warn!(doc = %doc_id, "local replay failed: {err}");
                            applied = false;
                            break;
                        }
                    }
                    if !applied {
                        shadow.dirty = true;
                        last_err = OverleafError::OutOfSync(
                            "local replay of confirmed op failed".to_string(),
                        );
                        continue;
                    }
                    shadow.version = applied_v + 1;
                    return Ok(EditOutcome {
                        content: shadow.content.clone(),
                        version: shadow.version,
                        changed: true,
                    });
                }
            }
        }
        Err(last_err)
    }

    async fn clear_in_flight(&self, doc_id: &str) {
        let mut st = self.inner.state.lock().await;
        if let Some(shadow) = st.docs.get_mut(doc_id) {
            shadow.in_flight = None;
            shadow.dirty = true;
        }
    }
}
