use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{OverleafError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
    #[serde(rename = "accessLevel", default)]
    pub access_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectList {
    pub projects: Vec<ProjectInfo>,
}

/// First argument of the `joinProjectResponse` realtime event.
#[derive(Debug, Clone, Deserialize)]
pub struct JoinProjectArgs {
    #[serde(rename = "publicId", default)]
    pub public_id: Option<String>,
    pub project: JoinedProject,
    #[serde(rename = "permissionsLevel", default)]
    pub permissions_level: Option<String>,
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JoinedProject {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
    #[serde(rename = "rootDoc_id", default)]
    pub root_doc_id: Option<String>,
    #[serde(rename = "rootFolder")]
    pub root_folder: Vec<FolderJson>,
    #[serde(default)]
    pub compiler: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FolderJson {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub folders: Vec<FolderJson>,
    #[serde(default)]
    pub docs: Vec<EntityRefJson>,
    #[serde(rename = "fileRefs", default)]
    pub file_refs: Vec<EntityRefJson>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityRefJson {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub entity_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EntityKind {
    Doc,
    File,
    Folder,
}

impl EntityKind {
    /// Path segment used by the web entity routes (rename/move/delete).
    pub fn route_segment(self) -> &'static str {
        match self {
            EntityKind::Doc => "doc",
            EntityKind::File => "file",
            EntityKind::Folder => "folder",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            EntityKind::Doc => "doc",
            EntityKind::File => "file",
            EntityKind::Folder => "folder",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub kind: EntityKind,
    /// Parent folder entity id; `None` when the parent is the root folder.
    pub parent: Option<String>,
}

/// Flat view of a project file tree, keyed by entity id. Paths are computed on
/// demand by walking parents, which keeps rename/move/remove trivial to apply
/// from realtime events.
#[derive(Debug, Clone)]
pub struct ProjectTree {
    pub project_id: String,
    pub project_name: String,
    pub root_folder_id: String,
    pub root_doc_id: Option<String>,
    pub compiler: Option<String>,
    nodes: BTreeMap<String, TreeNode>,
}

impl ProjectTree {
    pub fn from_join(project: &JoinedProject) -> Result<Self> {
        let root = project.root_folder.first().ok_or_else(|| {
            OverleafError::Protocol("joinProjectResponse has no root folder".to_string())
        })?;
        let mut tree = ProjectTree {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            root_folder_id: root.id.clone(),
            root_doc_id: project.root_doc_id.clone(),
            compiler: project.compiler.clone(),
            nodes: BTreeMap::new(),
        };
        // The root folder itself is not a node; its children have parent None.
        fn collect(tree: &mut ProjectTree, folder: &FolderJson, parent: Option<&str>) {
            for doc in &folder.docs {
                tree.nodes.insert(
                    doc.id.clone(),
                    TreeNode {
                        name: doc.name.clone(),
                        kind: EntityKind::Doc,
                        parent: parent.map(str::to_string),
                    },
                );
            }
            for file in &folder.file_refs {
                tree.nodes.insert(
                    file.id.clone(),
                    TreeNode {
                        name: file.name.clone(),
                        kind: EntityKind::File,
                        parent: parent.map(str::to_string),
                    },
                );
            }
            for sub in &folder.folders {
                tree.nodes.insert(
                    sub.id.clone(),
                    TreeNode {
                        name: sub.name.clone(),
                        kind: EntityKind::Folder,
                        parent: parent.map(str::to_string),
                    },
                );
                collect(tree, sub, Some(&sub.id));
            }
        }
        collect(&mut tree, root, None);
        Ok(tree)
    }

    /// Normalizes a user-supplied path to `/a/b/c` form.
    pub fn normalize_path(path: &str) -> String {
        let mut out = String::from("/");
        for seg in path.split('/') {
            let seg = seg.trim();
            if seg.is_empty() || seg == "." {
                continue;
            }
            if !out.ends_with('/') {
                out.push('/');
            }
            out.push_str(seg);
        }
        out
    }

    pub fn path_of(&self, id: &str) -> Option<String> {
        let mut segments = Vec::new();
        let mut cursor = self.nodes.get(id)?;
        segments.push(cursor.name.clone());
        while let Some(parent_id) = &cursor.parent {
            cursor = self.nodes.get(parent_id)?;
            segments.push(cursor.name.clone());
        }
        segments.reverse();
        Some(format!("/{}", segments.join("/")))
    }

    pub fn lookup(&self, path: &str) -> Option<(String, EntityKind)> {
        let wanted = Self::normalize_path(path);
        self.nodes.iter().find_map(|(id, node)| {
            (self.path_of(id)? == wanted).then(|| (id.clone(), node.kind))
        })
    }

    /// Folder entity id for a normalized folder path; `/` maps to the root folder.
    pub fn folder_id_of(&self, path: &str) -> Option<String> {
        let wanted = Self::normalize_path(path);
        if wanted == "/" {
            return Some(self.root_folder_id.clone());
        }
        match self.lookup(&wanted) {
            Some((id, EntityKind::Folder)) => Some(id),
            _ => None,
        }
    }

    /// All entities as `(path, kind, id)`, sorted by path.
    pub fn entries(&self) -> Vec<(String, EntityKind, String)> {
        let mut out: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|(id, node)| Some((self.path_of(id)?, node.kind, id.clone())))
            .collect();
        out.sort();
        out
    }

    pub fn docs(&self) -> Vec<(String, String)> {
        self.entries()
            .into_iter()
            .filter_map(|(path, kind, id)| (kind == EntityKind::Doc).then_some((path, id)))
            .collect()
    }

    pub fn insert_node(&mut self, id: String, name: String, kind: EntityKind, parent_id: &str) {
        let parent = (parent_id != self.root_folder_id).then(|| parent_id.to_string());
        self.nodes.insert(id, TreeNode { name, kind, parent });
    }

    /// Removes an entity and (for folders) everything below it. Returns the
    /// removed entity ids so callers can drop attached state such as doc shadows.
    pub fn remove_node(&mut self, id: &str) -> Vec<String> {
        let mut removed = Vec::new();
        if self.nodes.remove(id).is_some() {
            removed.push(id.to_string());
        }
        loop {
            let fresh: Vec<String> = self
                .nodes
                .iter()
                .filter(|(_, node)| {
                    node.parent
                        .as_deref()
                        .is_some_and(|p| removed.iter().any(|r| r == p))
                })
                .map(|(nid, _)| nid.clone())
                .filter(|nid| !removed.contains(nid))
                .collect();
            if fresh.is_empty() {
                break;
            }
            for nid in fresh {
                self.nodes.remove(&nid);
                removed.push(nid);
            }
        }
        removed
    }

    pub fn rename_node(&mut self, id: &str, name: String) -> bool {
        match self.nodes.get_mut(id) {
            Some(node) => {
                node.name = name;
                true
            }
            None => false,
        }
    }

    pub fn reparent_node(&mut self, id: &str, folder_id: &str) -> bool {
        let parent = (folder_id != self.root_folder_id).then(|| folder_id.to_string());
        match self.nodes.get_mut(id) {
            Some(node) => {
                node.parent = parent;
                true
            }
            None => false,
        }
    }

    pub fn kind_of(&self, id: &str) -> Option<EntityKind> {
        self.nodes.get(id).map(|node| node.kind)
    }
}
