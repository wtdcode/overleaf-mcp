use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use overleaf_client::OverleafClient;
use overleaf_realtime::ProjectConnection;
use overleaf_types::{
    CompileRequest, CompileResponse, EntityKind, OtComponent, OverleafConfig, OverleafError,
    ProjectInfo, ProjectTree, RealtimeSettings, Result,
};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct WorkspaceSettings {
    pub default_project: Option<String>,
    pub realtime: RealtimeSettings,
    pub search_max_matches: usize,
}

/// Errors and warnings distilled from a LaTeX compile log.
struct LogSummary {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl LogSummary {
    fn parse(log: &str, max_each: usize) -> Self {
        let lines: Vec<&str> = log.lines().collect();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut idx = 0;
        while idx < lines.len() {
            let line = lines[idx];
            if line.starts_with('!') && errors.len() < max_each {
                let mut block = vec![line.to_string()];
                let mut look = idx + 1;
                while look < lines.len() && look <= idx + 6 && !lines[look].trim().is_empty() {
                    block.push(lines[look].to_string());
                    look += 1;
                }
                errors.push(block.join("\n    "));
                idx = look;
                continue;
            }
            if (line.contains("LaTeX Warning:")
                || (line.starts_with("Package") && line.contains("Warning:")))
                && warnings.len() < max_each
            {
                warnings.push(line.trim().to_string());
            }
            idx += 1;
        }
        LogSummary { errors, warnings }
    }

    fn render(&self, out: &mut String) {
        if !self.errors.is_empty() {
            out.push_str(&format!("\nErrors ({}):\n", self.errors.len()));
            for err in &self.errors {
                out.push_str(&format!("  {err}\n"));
            }
        }
        if !self.warnings.is_empty() {
            out.push_str(&format!("\nWarnings ({}):\n", self.warnings.len()));
            for warn in &self.warnings {
                out.push_str(&format!("  {warn}\n"));
            }
        }
    }
}

/// Everything needed to act on one project path: the resolved project, a live
/// realtime connection, a fresh tree snapshot, the normalized path, and the
/// entity found there (if any).
struct Located {
    project: ProjectInfo,
    conn: ProjectConnection,
    tree: ProjectTree,
    path: String,
    entity: Option<(String, EntityKind)>,
}

/// Shared state behind all MCP tools: the HTTP client, one realtime connection
/// per touched project, the read-before-edit bookkeeping, and the most recent
/// compile result per project. Everything lives in memory; a restart simply
/// starts clean, which also re-imposes the read-before-edit discipline.
pub struct Workspace {
    client: Arc<OverleafClient>,
    settings: WorkspaceSettings,
    /// When set, the server is sandboxed to this project: every tool call
    /// naming another project is denied and list_projects shows only this one.
    pinned: Option<ProjectInfo>,
    conns: Mutex<BTreeMap<String, ProjectConnection>>,
    projects: Mutex<Vec<ProjectInfo>>,
    /// (project id, normalized path) -> full content as of the model's last
    /// read (or its own last clean edit). Overwrites insist on freshness
    /// against it, and edits diff against it to point out collaborator
    /// changes by line range.
    reads: Mutex<BTreeMap<(String, String), String>>,
    compiles: Mutex<BTreeMap<String, CompileRecord>>,
}

/// Result of the most recent compile of a project, kept so output files can be
/// fetched later and the (potentially huge) log can be paged through with
/// read_log instead of being dumped into one tool response.
struct CompileRecord {
    response: CompileResponse,
    log: Option<String>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("endpoint", &self.client.endpoint())
            .finish()
    }
}

impl Workspace {
    pub async fn connect(cfg: OverleafConfig, settings: WorkspaceSettings) -> Result<Arc<Self>> {
        let client = Arc::new(OverleafClient::new(cfg)?);
        client.login().await?;
        let projects = client.list_projects().await?.projects;
        tracing::info!("connected; {} projects visible", projects.len());
        let pinned = match settings.default_project.as_deref().map(str::trim) {
            Some(wanted) if !wanted.is_empty() => {
                let hit = Self::match_project(&projects, wanted).ok_or_else(|| {
                    OverleafError::NotFound(format!(
                        "pinned project `{wanted}` was not found on the server"
                    ))
                })?;
                tracing::info!(
                    "restricted to project '{}' ({})",
                    hit.name,
                    hit.id
                );
                Some(hit)
            }
            _ => None,
        };
        Ok(Arc::new(Workspace {
            client,
            settings,
            pinned,
            conns: Mutex::new(BTreeMap::new()),
            projects: Mutex::new(projects),
            reads: Mutex::new(BTreeMap::new()),
            compiles: Mutex::new(BTreeMap::new()),
        }))
    }

    pub fn pinned_project(&self) -> Option<&ProjectInfo> {
        self.pinned.as_ref()
    }

    fn match_project(projects: &[ProjectInfo], wanted: &str) -> Option<ProjectInfo> {
        if let Some(hit) = projects.iter().find(|p| p.id == wanted) {
            return Some(hit.clone());
        }
        if let Some(hit) = projects.iter().find(|p| p.name == wanted) {
            return Some(hit.clone());
        }
        let ci: Vec<&ProjectInfo> = projects
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(wanted))
            .collect();
        match ci.as_slice() {
            [only] => Some((*only).clone()),
            _ => None,
        }
    }

    /// 1-based inclusive line range of `after` that differs from `before`,
    /// via common prefix/suffix lines. Scattered changes collapse into one
    /// covering range; a pure deletion points at the join line. `None` when
    /// the contents are identical.
    fn changed_line_range(before: &str, after: &str) -> Option<(usize, usize)> {
        if before == after {
            return None;
        }
        let before: Vec<&str> = before.split('\n').collect();
        let after: Vec<&str> = after.split('\n').collect();
        let max_common = before.len().min(after.len());
        let prefix = before
            .iter()
            .zip(after.iter())
            .take_while(|(b, a)| b == a)
            .count();
        let suffix = before
            .iter()
            .rev()
            .zip(after.iter().rev())
            .take_while(|(b, a)| b == a)
            .count()
            .min(max_common - prefix);
        let start = prefix + 1;
        let end = after.len() - suffix;
        match end >= start {
            true => Some((start, end)),
            false => Some((start.saturating_sub(1).max(1), start.saturating_sub(1).max(1))),
        }
    }

    /// Overleaf's doc pipeline cannot store characters outside the Unicode
    /// BMP: OT inserts get mangled to U+FFFD server-side and the upload route
    /// demotes such text files to binary. Rejecting them up front keeps us
    /// from silently corrupting content.
    fn reject_non_bmp(text: &str, what: &str) -> Result<()> {
        match text.chars().find(|c| *c as u32 > 0xFFFF) {
            Some(ch) => Err(OverleafError::Edit(format!(
                "{what} contains {ch:?}, a character outside the Unicode BMP; Overleaf documents cannot store it (emoji etc.) — replace it, e.g. with a LaTeX escape, and retry"
            ))),
            None => Ok(()),
        }
    }

    async fn resolve_project(&self, wanted: Option<&str>) -> Result<ProjectInfo> {
        let wanted = wanted.map(str::trim).filter(|w| !w.is_empty());
        if let Some(pinned) = &self.pinned {
            return match wanted {
                None => Ok(pinned.clone()),
                Some(w) if w == pinned.id || w.eq_ignore_ascii_case(&pinned.name) => {
                    Ok(pinned.clone())
                }
                Some(w) => Err(OverleafError::Denied(format!(
                    "this server only has access to project '{}' ({}); `{w}` is not permitted — omit the `project` parameter",
                    pinned.name, pinned.id
                ))),
            };
        }
        let wanted = wanted.ok_or_else(|| {
            OverleafError::Edit(
                "no project given and the server has no default; pass `project` (see list_projects)"
                    .to_string(),
            )
        })?;
        for refresh in [false, true] {
            if refresh {
                let fresh = self.client.list_projects().await?.projects;
                *self.projects.lock().await = fresh;
            }
            let projects = self.projects.lock().await.clone();
            if let Some(hit) = Self::match_project(&projects, wanted) {
                return Ok(hit);
            }
        }
        Err(OverleafError::NotFound(format!(
            "project `{wanted}` (try list_projects)"
        )))
    }

    async fn conn(&self, project_id: &str) -> Result<ProjectConnection> {
        let mut conns = self.conns.lock().await;
        if let Some(existing) = conns.get(project_id) {
            if existing.is_alive() {
                return Ok(existing.clone());
            }
            conns.remove(project_id);
        }
        let fresh = ProjectConnection::open(
            self.client.clone(),
            project_id,
            self.settings.realtime.clone(),
        )
        .await?;
        conns.insert(project_id.to_string(), fresh.clone());
        Ok(fresh)
    }

    /// Tree snapshot with one automatic reconnect when the connection lost
    /// track of the tree (rare structural races).
    async fn project_tree(&self, project_id: &str) -> Result<(ProjectConnection, ProjectTree)> {
        let conn = self.conn(project_id).await?;
        match conn.tree().await {
            Ok(tree) => Ok((conn, tree)),
            Err(OverleafError::OutOfSync(_)) => {
                conn.shutdown().await;
                self.conns.lock().await.remove(project_id);
                let conn = self.conn(project_id).await?;
                let tree = conn.tree().await?;
                Ok((conn, tree))
            }
            Err(err) => Err(err),
        }
    }

    async fn locate(&self, project: Option<&str>, path: &str) -> Result<Located> {
        let project = self.resolve_project(project).await?;
        let (conn, tree) = self.project_tree(&project.id).await?;
        let path = ProjectTree::normalize_path(path);
        let entity = tree.lookup(&path);
        Ok(Located {
            project,
            conn,
            tree,
            path,
            entity,
        })
    }

    async fn stamp_read(&self, project_id: &str, path: &str, content: &str) {
        self.reads.lock().await.insert(
            (project_id.to_string(), path.to_string()),
            content.to_string(),
        );
    }

    async fn read_stamp(&self, project_id: &str, path: &str) -> Option<String> {
        self.reads
            .lock()
            .await
            .get(&(project_id.to_string(), path.to_string()))
            .cloned()
    }

    async fn drop_stamps_under(&self, project_id: &str, path: &str) {
        let prefix = format!("{path}/");
        self.reads.lock().await.retain(|(pid, p), _| {
            pid != project_id || (p != path && !p.starts_with(&prefix))
        });
    }

    /// Byte-level common prefix/suffix trim producing at most one replacement,
    /// so a full overwrite becomes a compact op and an identical write a no-op.
    fn full_replace_edits(current: &str, target: &str) -> Vec<(usize, usize, String)> {
        if current == target {
            return Vec::new();
        }
        let mut prefix = current
            .bytes()
            .zip(target.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !current.is_char_boundary(prefix) || !target.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let cur_rest = &current[prefix..];
        let tgt_rest = &target[prefix..];
        let mut suffix = cur_rest
            .bytes()
            .rev()
            .zip(tgt_rest.bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while suffix > 0
            && (!cur_rest.is_char_boundary(cur_rest.len() - suffix)
                || !tgt_rest.is_char_boundary(tgt_rest.len() - suffix))
        {
            suffix -= 1;
        }
        vec![(
            prefix,
            current.len() - suffix,
            tgt_rest[..tgt_rest.len() - suffix].to_string(),
        )]
    }

    fn render_numbered(content: &str, offset: usize, limit: Option<usize>) -> (String, usize) {
        let lines: Vec<&str> = content.split('\n').collect();
        let total = lines.len();
        let start = offset.max(1) - 1;
        let end = match limit {
            Some(limit) => (start + limit).min(total),
            None => total,
        };
        let mut out = String::new();
        for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
            out.push_str(&format!("{:>6}\t{}\n", idx + 1, line));
        }
        (out, total)
    }

    /// Creates any missing folders along `folder_path` and returns its id.
    async fn ensure_folder(
        &self,
        project_id: &str,
        conn: &ProjectConnection,
        tree: &ProjectTree,
        folder_path: &str,
    ) -> Result<String> {
        let wanted = ProjectTree::normalize_path(folder_path);
        let mut tree = tree.clone();
        if let Some(id) = tree.folder_id_of(&wanted) {
            return Ok(id);
        }
        let mut current_path = String::new();
        let mut current_id = tree.root_folder_id.clone();
        for segment in wanted.split('/').filter(|s| !s.is_empty()) {
            current_path.push('/');
            current_path.push_str(segment);
            match tree.lookup(&current_path) {
                Some((id, EntityKind::Folder)) => current_id = id,
                Some((_, kind)) => {
                    return Err(OverleafError::Edit(format!(
                        "{current_path} exists and is a {}, not a folder",
                        kind.describe()
                    )));
                }
                None => {
                    let created = self
                        .client
                        .create_folder(project_id, &current_id, segment)
                        .await?;
                    conn.tree_apply_created(
                        &current_id,
                        created.id.clone(),
                        created.name.clone(),
                        EntityKind::Folder,
                    )
                    .await;
                    tree.insert_node(
                        created.id.clone(),
                        created.name,
                        EntityKind::Folder,
                        &current_id,
                    );
                    current_id = created.id;
                }
            }
        }
        Ok(current_id)
    }

    async fn write_local(&self, local_path: &str, bytes: &[u8]) -> Result<()> {
        let target = Path::new(local_path);
        if let Some(parent) = target.parent()
            && !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        // Write-then-rename so an interrupted download never leaves a torn file.
        let tmp = target.with_extension("overleaf-mcp.tmp");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, target).await?;
        Ok(())
    }

    // --- tool entry points -------------------------------------------------

    pub async fn list_projects(&self) -> Result<String> {
        if let Some(pinned) = &self.pinned {
            let access = pinned.access_level.as_deref().unwrap_or("member");
            return Ok(format!(
                "This server is restricted to a single project:\n{}  {} ({access})\nOmit the `project` parameter in tool calls; other projects are not accessible.\n",
                pinned.id, pinned.name
            ));
        }
        let fresh = self.client.list_projects().await?.projects;
        let mut out = format!("{} projects on {}:\n", fresh.len(), self.client.endpoint());
        for p in &fresh {
            let access = p.access_level.as_deref().unwrap_or("member");
            out.push_str(&format!("{}  {} ({})\n", p.id, p.name, access));
        }
        *self.projects.lock().await = fresh;
        Ok(out)
    }

    pub async fn list_files(&self, project: Option<&str>) -> Result<String> {
        let project = self.resolve_project(project).await?;
        let (_, tree) = self.project_tree(&project.id).await?;
        let mut out = format!(
            "Project {} ({}), compiler: {}\n",
            tree.project_name,
            tree.project_id,
            tree.compiler.as_deref().unwrap_or("unknown")
        );
        for (path, kind, id) in tree.entries() {
            let root_marker = match (&tree.root_doc_id, kind) {
                (Some(root), EntityKind::Doc) if *root == id => " [root document]",
                _ => "",
            };
            out.push_str(&format!("{path} ({}){root_marker}\n", kind.describe()));
        }
        Ok(out)
    }

    pub async fn read_file(
        &self,
        project: Option<&str>,
        path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String> {
        let loc = self.locate(project, path).await?;
        match loc.entity {
            Some((doc_id, EntityKind::Doc)) => {
                let (content, version) = loc.conn.read_doc(&doc_id).await?;
                self.stamp_read(&loc.project.id, &loc.path, &content).await;
                let (body, total) =
                    Self::render_numbered(&content, offset.unwrap_or(1), limit);
                let mut out = format!(
                    "{} (doc, version {version}, {total} lines)\n",
                    loc.path
                );
                out.push_str(&body);
                Ok(out)
            }
            Some((_, EntityKind::File)) => Ok(format!(
                "{} is a binary file; use download_file to save it locally.",
                loc.path
            )),
            Some((_, EntityKind::Folder)) => Err(OverleafError::Edit(format!(
                "{} is a folder; use list_files",
                loc.path
            ))),
            None => Err(OverleafError::NotFound(format!(
                "{} in project {}",
                loc.path, loc.project.name
            ))),
        }
    }

    /// Metadata only. Deliberately does not count as a read: the model has not
    /// seen the content, so the read-before-edit discipline stays intact.
    pub async fn stat_file(&self, project: Option<&str>, path: &str) -> Result<String> {
        let loc = self.locate(project, path).await?;
        // The root folder is not a tree node; treat it as the whole project.
        let entity = match loc.path.as_str() {
            "/" => Some((loc.tree.root_folder_id.clone(), EntityKind::Folder)),
            _ => loc.entity,
        };
        match entity {
            Some((doc_id, EntityKind::Doc)) => {
                let (content, version) = loc.conn.read_doc(&doc_id).await?;
                Ok(format!(
                    "{}: doc, version {version}, {} lines, {} bytes",
                    loc.path,
                    content.split('\n').count(),
                    content.len()
                ))
            }
            Some((file_id, EntityKind::File)) => {
                let size = self.client.file_size(&loc.project.id, &file_id).await?;
                let size = size
                    .map(|s| format!("{s} bytes"))
                    .unwrap_or_else(|| "size unknown".to_string());
                Ok(format!("{}: binary file, {size}", loc.path))
            }
            Some((_, EntityKind::Folder)) => {
                let prefix = match loc.path.as_str() {
                    "/" => "/".to_string(),
                    p => format!("{p}/"),
                };
                let (mut docs, mut files, mut folders) = (0usize, 0usize, 0usize);
                for (entry_path, kind, _) in loc.tree.entries() {
                    if entry_path.starts_with(&prefix) {
                        match kind {
                            EntityKind::Doc => docs += 1,
                            EntityKind::File => files += 1,
                            EntityKind::Folder => folders += 1,
                        }
                    }
                }
                Ok(format!(
                    "{}: folder containing {docs} doc(s), {files} binary file(s), {folders} subfolder(s)",
                    loc.path
                ))
            }
            None => Err(OverleafError::NotFound(format!(
                "{} in project {}",
                loc.path, loc.project.name
            ))),
        }
    }

    pub async fn edit_file(
        &self,
        project: Option<&str>,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<String> {
        if old_string.is_empty() {
            return Err(OverleafError::Edit(
                "old_string must not be empty".to_string(),
            ));
        }
        if old_string == new_string {
            return Err(OverleafError::Edit(
                "old_string and new_string are identical".to_string(),
            ));
        }
        Self::reject_non_bmp(new_string, "new_string")?;
        let loc = self.locate(project, path).await?;
        let (doc_id, kind) = loc.entity.clone().ok_or_else(|| {
            OverleafError::NotFound(format!("{} in project {}", loc.path, loc.project.name))
        })?;
        if kind != EntityKind::Doc {
            return Err(OverleafError::Edit(format!(
                "{} is a {}, not an editable doc",
                loc.path,
                kind.describe()
            )));
        }
        let last_read = self
            .read_stamp(&loc.project.id, &loc.path)
            .await
            .ok_or_else(|| {
                OverleafError::Edit(format!(
                    "{} has not been read in this session; call read_file first",
                    loc.path
                ))
            })?;
        let mut match_count = 0usize;
        let mut first_offset = 0usize;
        let mut pre_content = String::new();
        let outcome = loc
            .conn
            .edit_doc(&doc_id, |content| {
                pre_content = content.to_string();
                let offsets: Vec<usize> = content
                    .match_indices(old_string)
                    .map(|(off, _)| off)
                    .collect();
                match_count = offsets.len();
                match offsets.as_slice() {
                    [] => Err(OverleafError::Edit(format!(
                        "old_string not found in the current content of {}; the doc may have changed — call read_file again",
                        loc.path
                    ))),
                    [single] => {
                        first_offset = *single;
                        let edits = vec![(
                            *single,
                            *single + old_string.len(),
                            new_string.to_string(),
                        )];
                        Ok(OtComponent::replace_script(content, &edits))
                    }
                    many if replace_all => {
                        first_offset = many[0];
                        let edits: Vec<(usize, usize, String)> = many
                            .iter()
                            .map(|off| (*off, *off + old_string.len(), new_string.to_string()))
                            .collect();
                        Ok(OtComponent::replace_script(content, &edits))
                    }
                    many => Err(OverleafError::Edit(format!(
                        "old_string appears {} times in {}; add surrounding context to make it unique or set replace_all",
                        many.len(),
                        loc.path
                    ))),
                }
            })
            .await?;
        // The stamp advances only for a clean edit. When collaborators changed
        // the doc since the last read, the edit still applied (old_string
        // anchored it), but the model is told and keeps being told until it
        // actually re-reads.
        let drift = Self::changed_line_range(&last_read, &pre_content);
        if drift.is_none() {
            self.stamp_read(&loc.project.id, &loc.path, &outcome.content)
                .await;
        }
        let line = outcome.content[..first_offset.min(outcome.content.len())]
            .matches('\n')
            .count()
            + 1;
        let span = new_string.matches('\n').count() + 1;
        let from = line.saturating_sub(3).max(1);
        let (snippet, _) =
            Self::render_numbered(&outcome.content, from, Some(span + 6));
        let mut out = format!(
            "Replaced {match_count} occurrence(s) in {} (now version {}).\n{snippet}",
            loc.path, outcome.version
        );
        if let Some((start, end)) = drift {
            out.push_str(&format!(
                "\nNote: other collaborators edited {} since your last read (around line{} {} of the current content). Your edit was applied, but call read_file to see their changes before editing further.\n",
                loc.path,
                if end > start { "s" } else { "" },
                if end > start {
                    format!("{start}-{end}")
                } else {
                    format!("{start}")
                }
            ));
        }
        Ok(out)
    }

    pub async fn write_file(
        &self,
        project: Option<&str>,
        path: &str,
        content: &str,
    ) -> Result<String> {
        Self::reject_non_bmp(content, "content")?;
        let loc = self.locate(project, path).await?;
        match loc.entity.clone() {
            Some((doc_id, EntityKind::Doc)) => {
                let (current, _) = loc.conn.read_doc(&doc_id).await?;
                let stamp = self.read_stamp(&loc.project.id, &loc.path).await;
                if stamp.as_deref() != Some(current.as_str()) {
                    return Err(OverleafError::Edit(format!(
                        "{} was not read since its last change; call read_file first and merge your changes",
                        loc.path
                    )));
                }
                let outcome = loc
                    .conn
                    .edit_doc(&doc_id, |current| {
                        Ok(OtComponent::replace_script(
                            current,
                            &Self::full_replace_edits(current, content),
                        ))
                    })
                    .await?;
                self.stamp_read(&loc.project.id, &loc.path, &outcome.content)
                    .await;
                match outcome.changed {
                    true => Ok(format!(
                        "Overwrote {} ({} lines, version {}).",
                        loc.path,
                        outcome.content.split('\n').count(),
                        outcome.version
                    )),
                    false => Ok(format!("{} already had this content.", loc.path)),
                }
            }
            Some((_, kind)) => Err(OverleafError::Edit(format!(
                "{} is a {}; write_file only handles docs (use upload_file for binary assets)",
                loc.path,
                kind.describe()
            ))),
            None => {
                let (folder_path, name) = match loc.path.rsplit_once('/') {
                    Some((dir, name)) if !name.is_empty() => (
                        if dir.is_empty() { "/" } else { dir }.to_string(),
                        name.to_string(),
                    ),
                    _ => {
                        return Err(OverleafError::Edit(format!(
                            "{} is not a valid file path",
                            loc.path
                        )));
                    }
                };
                let folder_id = self
                    .ensure_folder(&loc.project.id, &loc.conn, &loc.tree, &folder_path)
                    .await?;
                let created = self
                    .client
                    .create_doc(&loc.project.id, &folder_id, &name)
                    .await?;
                loc.conn
                    .tree_apply_created(
                        &folder_id,
                        created.id.clone(),
                        created.name.clone(),
                        EntityKind::Doc,
                    )
                    .await;
                let outcome = loc
                    .conn
                    .edit_doc(&created.id, |current| {
                        Ok(OtComponent::replace_script(
                            current,
                            &Self::full_replace_edits(current, content),
                        ))
                    })
                    .await?;
                self.stamp_read(&loc.project.id, &loc.path, &outcome.content)
                    .await;
                Ok(format!(
                    "Created {} ({} lines, version {}).",
                    loc.path,
                    outcome.content.split('\n').count(),
                    outcome.version
                ))
            }
        }
    }

    pub async fn search(
        &self,
        project: Option<&str>,
        pattern: &str,
        path: Option<&str>,
        case_insensitive: bool,
    ) -> Result<String> {
        let regex = regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| OverleafError::Edit(format!("invalid regex: {e}")))?;
        let project = self.resolve_project(project).await?;
        let (conn, tree) = self.project_tree(&project.id).await?;
        let scope = path.map(ProjectTree::normalize_path);
        let docs: Vec<(String, String)> = tree
            .docs()
            .into_iter()
            .filter(|(doc_path, _)| match &scope {
                None => true,
                Some(scope) if scope == "/" => true,
                Some(scope) => {
                    doc_path == scope || doc_path.starts_with(&format!("{scope}/"))
                }
            })
            .collect();
        if docs.is_empty() {
            return Err(OverleafError::NotFound(format!(
                "no docs to search under {} in project {}",
                scope.as_deref().unwrap_or("/"),
                project.name
            )));
        }
        let mut set = tokio::task::JoinSet::new();
        for (doc_path, doc_id) in docs {
            let conn = conn.clone();
            set.spawn(async move {
                let read = conn.read_doc(&doc_id).await;
                (doc_path, read)
            });
        }
        let mut contents: BTreeMap<String, String> = BTreeMap::new();
        while let Some(joined) = set.join_next().await {
            let (doc_path, read) = joined.map_err(|e| {
                OverleafError::Protocol(format!("search task failed: {e}"))
            })?;
            contents.insert(doc_path, read?.0);
        }
        let mut hits = 0usize;
        let mut truncated = false;
        let mut out = String::new();
        'outer: for (doc_path, content) in &contents {
            for (idx, line) in content.split('\n').enumerate() {
                if regex.is_match(line) {
                    if hits >= self.settings.search_max_matches {
                        truncated = true;
                        break 'outer;
                    }
                    let shown: String = line.chars().take(200).collect();
                    out.push_str(&format!("{doc_path}:{}: {shown}\n", idx + 1));
                    hits += 1;
                }
            }
        }
        let mut header = format!(
            "{hits} match(es) for `{pattern}` across {} doc(s)",
            contents.len()
        );
        if truncated {
            header.push_str(" (truncated)");
        }
        header.push('\n');
        header.push_str(&out);
        Ok(header)
    }

    pub async fn create_folder(&self, project: Option<&str>, path: &str) -> Result<String> {
        let loc = self.locate(project, path).await?;
        let folder_id = self
            .ensure_folder(&loc.project.id, &loc.conn, &loc.tree, &loc.path)
            .await?;
        Ok(format!("Folder {} ready (id {folder_id}).", loc.path))
    }

    pub async fn delete_entity(&self, project: Option<&str>, path: &str) -> Result<String> {
        let loc = self.locate(project, path).await?;
        let (entity_id, kind) = loc.entity.ok_or_else(|| {
            OverleafError::NotFound(format!("{} in project {}", loc.path, loc.project.name))
        })?;
        self.client
            .delete_entity(&loc.project.id, kind, &entity_id)
            .await?;
        loc.conn.tree_apply_removed(&entity_id).await;
        self.drop_stamps_under(&loc.project.id, &loc.path).await;
        Ok(format!("Deleted {} {}.", kind.describe(), loc.path))
    }

    pub async fn rename_entity(
        &self,
        project: Option<&str>,
        path: &str,
        new_name: &str,
    ) -> Result<String> {
        if new_name.contains('/') || new_name.trim().is_empty() {
            return Err(OverleafError::Edit(
                "new_name must be a plain name without slashes".to_string(),
            ));
        }
        let loc = self.locate(project, path).await?;
        let (entity_id, kind) = loc.entity.ok_or_else(|| {
            OverleafError::NotFound(format!("{} in project {}", loc.path, loc.project.name))
        })?;
        self.client
            .rename_entity(&loc.project.id, kind, &entity_id, new_name.trim())
            .await?;
        loc.conn
            .tree_apply_renamed(&entity_id, new_name.trim().to_string())
            .await;
        self.drop_stamps_under(&loc.project.id, &loc.path).await;
        Ok(format!(
            "Renamed {} {} to {}.",
            kind.describe(),
            loc.path,
            new_name.trim()
        ))
    }

    pub async fn move_entity(
        &self,
        project: Option<&str>,
        path: &str,
        target_folder: &str,
    ) -> Result<String> {
        let loc = self.locate(project, path).await?;
        let (entity_id, kind) = loc.entity.clone().ok_or_else(|| {
            OverleafError::NotFound(format!("{} in project {}", loc.path, loc.project.name))
        })?;
        let folder_id = self
            .ensure_folder(&loc.project.id, &loc.conn, &loc.tree, target_folder)
            .await?;
        self.client
            .move_entity(&loc.project.id, kind, &entity_id, &folder_id)
            .await?;
        loc.conn.tree_apply_moved(&entity_id, &folder_id).await;
        self.drop_stamps_under(&loc.project.id, &loc.path).await;
        Ok(format!(
            "Moved {} {} into {}.",
            kind.describe(),
            loc.path,
            ProjectTree::normalize_path(target_folder)
        ))
    }

    pub async fn upload_file(
        &self,
        project: Option<&str>,
        local_path: &str,
        project_path: &str,
    ) -> Result<String> {
        let bytes = tokio::fs::read(local_path).await?;
        let loc = self.locate(project, project_path).await?;
        let (folder_path, name) = match loc.path.rsplit_once('/') {
            Some((dir, name)) if !name.is_empty() => (
                if dir.is_empty() { "/" } else { dir }.to_string(),
                name.to_string(),
            ),
            _ => {
                return Err(OverleafError::Edit(format!(
                    "{} is not a valid file path",
                    loc.path
                )));
            }
        };
        let folder_id = self
            .ensure_folder(&loc.project.id, &loc.conn, &loc.tree, &folder_path)
            .await?;
        let size = bytes.len();
        let uploaded = self
            .client
            .upload_file(&loc.project.id, &folder_id, &name, bytes)
            .await?;
        let kind = match uploaded.entity_type.as_deref() {
            Some("doc") => EntityKind::Doc,
            Some("folder") => EntityKind::Folder,
            _ => EntityKind::File,
        };
        if let Some(entity_id) = uploaded.entity_id.clone() {
            loc.conn
                .tree_apply_created(&folder_id, entity_id, name.clone(), kind)
                .await;
        }
        self.drop_stamps_under(&loc.project.id, &loc.path).await;
        Ok(format!(
            "Uploaded {size} bytes to {} as a {}.",
            loc.path,
            kind.describe()
        ))
    }

    pub async fn download_file(
        &self,
        project: Option<&str>,
        path: &str,
        local_path: &str,
    ) -> Result<String> {
        let loc = self.locate(project, path).await?;
        let (entity_id, kind) = loc.entity.ok_or_else(|| {
            OverleafError::NotFound(format!("{} in project {}", loc.path, loc.project.name))
        })?;
        let bytes = match kind {
            EntityKind::Doc => {
                let (content, _) = loc.conn.read_doc(&entity_id).await?;
                content.into_bytes()
            }
            EntityKind::File => {
                self.client
                    .download_file(&loc.project.id, &entity_id)
                    .await?
            }
            EntityKind::Folder => {
                return Err(OverleafError::Edit(format!(
                    "{} is a folder; download files individually",
                    loc.path
                )));
            }
        };
        self.write_local(local_path, &bytes).await?;
        Ok(format!(
            "Saved {} ({} bytes) to {local_path}.",
            loc.path,
            bytes.len()
        ))
    }

    fn format_ts(ms: Option<i64>) -> String {
        ms.and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "unknown time".to_string())
    }

    pub async fn get_history(&self, project: Option<&str>, before: Option<i64>) -> Result<String> {
        let project = self.resolve_project(project).await?;
        if before.is_none() {
            // Push pending realtime edits into history first; best-effort since
            // a project without recent edits has nothing to flush.
            if let Err(err) = self.client.flush_history(&project.id).await {
                tracing::debug!("history flush failed: {err}");
            }
        }
        let updates = self
            .client
            .history_updates(&project.id, before, 25)
            .await?;
        let labels = self.client.history_labels(&project.id).await?;
        let mut out = format!(
            "History of {} ({} update batch(es), newest first):\n",
            project.name,
            updates.updates.len()
        );
        for u in &updates.updates {
            let users = u
                .meta
                .as_ref()
                .map(|m| {
                    m.users
                        .iter()
                        .map(|user| user.display())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let when = Self::format_ts(u.meta.as_ref().and_then(|m| m.end_ts));
            out.push_str(&format!("v{} -> v{}  {when}  by {users}\n", u.from_v, u.to_v));
            if !u.pathnames.is_empty() {
                out.push_str(&format!("    edited: {}\n", u.pathnames.join(", ")));
            }
            for op in &u.project_ops {
                let rendered = ["add", "remove", "rename"].iter().find_map(|kind| {
                    let body = op.get(kind)?;
                    let pathname = body.get("pathname")?.as_str()?;
                    Some(match *kind {
                        "rename" => format!(
                            "renamed {pathname} -> {}",
                            body.get("newPathname").and_then(serde_json::Value::as_str).unwrap_or("?")
                        ),
                        "remove" => format!("removed {pathname}"),
                        _ => format!("added {pathname}"),
                    })
                });
                match rendered {
                    Some(line) => out.push_str(&format!("    {line}\n")),
                    None => out.push_str(&format!("    {op}\n")),
                }
            }
            for label in &u.labels {
                out.push_str(&format!(
                    "    label at v{}: \"{}\"\n",
                    label.version, label.comment
                ));
            }
        }
        out.push_str("\nLabels:\n");
        match labels.as_slice() {
            [] => out.push_str("  (none)\n"),
            all => {
                for label in all {
                    out.push_str(&format!(
                        "  v{}  \"{}\"  ({})\n",
                        label.version,
                        label.comment,
                        label.created_at.as_deref().unwrap_or("unknown time")
                    ));
                }
            }
        }
        if let Some(next) = updates.next_before_timestamp {
            out.push_str(&format!(
                "\nOlder history available: call get_history with before={next}.\n"
            ));
        }
        Ok(out)
    }

    pub async fn label_version(
        &self,
        project: Option<&str>,
        version: i64,
        comment: &str,
    ) -> Result<String> {
        let comment = comment.trim();
        if comment.is_empty() {
            return Err(OverleafError::Edit(
                "label comment must not be empty".to_string(),
            ));
        }
        let project = self.resolve_project(project).await?;
        if let Err(err) = self.client.flush_history(&project.id).await {
            tracing::debug!("history flush failed: {err}");
        }
        let label = self
            .client
            .create_label(&project.id, version, comment)
            .await?;
        Ok(format!(
            "Created label \"{}\" at history version {} (id {}).",
            label.comment, label.version, label.id
        ))
    }

    pub async fn compile(&self, project: Option<&str>) -> Result<String> {
        let project = self.resolve_project(project).await?;
        let response = self
            .client
            .compile(&project.id, &CompileRequest::full(None))
            .await?;
        // Rate-limited responses carry no output files; keep the previous
        // compile's record so its log and artifacts stay reachable.
        if response.status == "too-recently-compiled" {
            return Ok(format!(
                "Compile of {} was rate-limited (too-recently-compiled): the project was compiled moments ago. Wait a few seconds and retry; the previous compile's outputs remain available via read_log/download_output.",
                project.name
            ));
        }
        let mut out = format!(
            "Compile of {} finished with status: {}\n",
            project.name, response.status
        );
        let log_file = response
            .output_files
            .iter()
            .find(|f| f.path == "output.log")
            .cloned();
        let has_pdf = response.output_files.iter().any(|f| f.path == "output.pdf");
        let mut log = None;
        if let Some(log_file) = log_file {
            match self
                .client
                .download_output(&log_file.url, response.clsi_server_id.as_deref())
                .await
            {
                Ok(log_bytes) => {
                    let log_text = String::from_utf8_lossy(&log_bytes).into_owned();
                    LogSummary::parse(&log_text, 10).render(&mut out);
                    out.push_str(&format!(
                        "\nFull log: {} lines — page through it with read_log if the summary is not enough.\n",
                        log_text.split('\n').count()
                    ));
                    log = Some(log_text);
                }
                Err(err) => {
                    out.push_str(&format!("(could not fetch output.log: {err})\n"));
                }
            }
        }
        out.push_str(&format!(
            "Output files: {}\n",
            response
                .output_files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if has_pdf {
            out.push_str("Use download_output to save output.pdf locally.\n");
        }
        self.compiles
            .lock()
            .await
            .insert(project.id, CompileRecord { response, log });
        Ok(out)
    }

    /// Pages through the output.log of the most recent compile. The log was
    /// already fetched during compile and is served from memory; it is only
    /// re-downloaded when that first fetch failed.
    pub async fn read_log(
        &self,
        project: Option<&str>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String> {
        let project = self.resolve_project(project).await?;
        let (status, log) = {
            let mut compiles = self.compiles.lock().await;
            let record = compiles.get_mut(&project.id).ok_or_else(|| {
                OverleafError::Edit(
                    "no compile result for this project yet; run compile first".to_string(),
                )
            })?;
            if record.log.is_none() {
                let log_file = record
                    .response
                    .output_files
                    .iter()
                    .find(|f| f.path == "output.log")
                    .cloned()
                    .ok_or_else(|| {
                        OverleafError::NotFound(
                            "output.log in the last compile".to_string(),
                        )
                    })?;
                let bytes = self
                    .client
                    .download_output(&log_file.url, record.response.clsi_server_id.as_deref())
                    .await?;
                record.log = Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            (
                record.response.status.clone(),
                record.log.clone().unwrap_or_default(),
            )
        };
        let (body, total) = Self::render_numbered(&log, offset.unwrap_or(1), Some(limit.unwrap_or(200)));
        let mut out = format!(
            "output.log of last compile of {} (status {status}, {total} lines total)\n",
            project.name
        );
        out.push_str(&body);
        Ok(out)
    }

    pub async fn download_output(
        &self,
        project: Option<&str>,
        file: &str,
        local_path: &str,
    ) -> Result<String> {
        let project = self.resolve_project(project).await?;
        let cached = self
            .compiles
            .lock()
            .await
            .get(&project.id)
            .map(|record| record.response.clone());
        let response = cached.ok_or_else(|| {
            OverleafError::Edit("no compile result for this project yet; run compile first".to_string())
        })?;
        let target = response
            .output_files
            .iter()
            .find(|f| f.path == file)
            .ok_or_else(|| {
                OverleafError::NotFound(format!(
                    "{file} in the last compile (available: {})",
                    response
                        .output_files
                        .iter()
                        .map(|f| f.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        let bytes = self
            .client
            .download_output(&target.url, response.clsi_server_id.as_deref())
            .await?;
        self.write_local(local_path, &bytes).await?;
        Ok(format!(
            "Saved {file} ({} bytes) to {local_path}.",
            bytes.len()
        ))
    }
}
