use std::sync::Arc;

use llmy::agent::tool::ToolBox;
use llmy::agent::{LLMYError, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::prompts;
use crate::workspace::Workspace;

// LLMYError is what the llmy tool layer requires; its size is llmy's business,
// so the large-variant lint is silenced rather than boxed around.
#[allow(clippy::result_large_err)]
trait LlmyResultExt<T> {
    fn into_llmy(self) -> Result<T, LLMYError>;
}

impl<T> LlmyResultExt<T> for overleaf_types::Result<T> {
    fn into_llmy(self) -> Result<T, LLMYError> {
        self.map_err(|err| LLMYError::Other(color_eyre::eyre::Report::new(err)))
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ListProjectsArgs {}

#[derive(Clone, Debug)]
pub struct ListProjectsTool {
    ws: Arc<Workspace>,
}

impl Tool for ListProjectsTool {
    type ARGUMENTS = ListProjectsArgs;
    const NAME: &str = "list_projects";
    const DESCRIPTION: Option<&str> = Some(prompts::LIST_PROJECTS);

    async fn invoke(&self, _args: ListProjectsArgs) -> Result<String, LLMYError> {
        self.ws.list_projects().await.into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ListFilesArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ListFilesTool {
    ws: Arc<Workspace>,
}

impl Tool for ListFilesTool {
    type ARGUMENTS = ListFilesArgs;
    const NAME: &str = "list_files";
    const DESCRIPTION: Option<&str> = Some(prompts::LIST_FILES);

    async fn invoke(&self, args: ListFilesArgs) -> Result<String, LLMYError> {
        self.ws.list_files(args.project.as_deref()).await.into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// File path inside the project, e.g. `/main.tex`.
    pub path: String,
    /// 1-based line to start reading from (default: 1).
    pub offset: Option<usize>,
    /// Maximum number of lines to return (default: all).
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ReadFileTool {
    ws: Arc<Workspace>,
}

impl Tool for ReadFileTool {
    type ARGUMENTS = ReadFileArgs;
    const NAME: &str = "read_file";
    const DESCRIPTION: Option<&str> = Some(prompts::READ_FILE);

    async fn invoke(&self, args: ReadFileArgs) -> Result<String, LLMYError> {
        self.ws
            .read_file(args.project.as_deref(), &args.path, args.offset, args.limit)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct StatFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Path of the doc, file, or folder to inspect.
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct StatFileTool {
    ws: Arc<Workspace>,
}

impl Tool for StatFileTool {
    type ARGUMENTS = StatFileArgs;
    const NAME: &str = "stat_file";
    const DESCRIPTION: Option<&str> = Some(prompts::STAT_FILE);

    async fn invoke(&self, args: StatFileArgs) -> Result<String, LLMYError> {
        self.ws
            .stat_file(args.project.as_deref(), &args.path)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct EditFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// File path inside the project, e.g. `/main.tex`.
    pub path: String,
    /// Exact text to replace; must match the current content.
    pub old_string: String,
    /// Replacement text.
    pub new_string: String,
    /// Replace every occurrence instead of requiring a unique match.
    pub replace_all: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct EditFileTool {
    ws: Arc<Workspace>,
}

impl Tool for EditFileTool {
    type ARGUMENTS = EditFileArgs;
    const NAME: &str = "edit_file";
    const DESCRIPTION: Option<&str> = Some(prompts::EDIT_FILE);

    async fn invoke(&self, args: EditFileArgs) -> Result<String, LLMYError> {
        self.ws
            .edit_file(
                args.project.as_deref(),
                &args.path,
                &args.old_string,
                &args.new_string,
                args.replace_all.unwrap_or(false),
            )
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// File path inside the project, e.g. `/sections/intro.tex`.
    pub path: String,
    /// Full new content of the file.
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct WriteFileTool {
    ws: Arc<Workspace>,
}

impl Tool for WriteFileTool {
    type ARGUMENTS = WriteFileArgs;
    const NAME: &str = "write_file";
    const DESCRIPTION: Option<&str> = Some(prompts::WRITE_FILE);

    async fn invoke(&self, args: WriteFileArgs) -> Result<String, LLMYError> {
        self.ws
            .write_file(args.project.as_deref(), &args.path, &args.content)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Regular expression (Rust regex syntax), matched line by line.
    pub pattern: String,
    /// Limit the search to one doc path or one folder path (default: whole project).
    pub path: Option<String>,
    /// Case-insensitive matching (default: false).
    pub case_insensitive: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct SearchTool {
    ws: Arc<Workspace>,
}

impl Tool for SearchTool {
    type ARGUMENTS = SearchArgs;
    const NAME: &str = "search";
    const DESCRIPTION: Option<&str> = Some(prompts::SEARCH);

    async fn invoke(&self, args: SearchArgs) -> Result<String, LLMYError> {
        self.ws
            .search(
                args.project.as_deref(),
                &args.pattern,
                args.path.as_deref(),
                args.case_insensitive.unwrap_or(false),
            )
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateFolderArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Folder path to create, e.g. `/figures/plots`.
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct CreateFolderTool {
    ws: Arc<Workspace>,
}

impl Tool for CreateFolderTool {
    type ARGUMENTS = CreateFolderArgs;
    const NAME: &str = "create_folder";
    const DESCRIPTION: Option<&str> = Some(prompts::CREATE_FOLDER);

    async fn invoke(&self, args: CreateFolderArgs) -> Result<String, LLMYError> {
        self.ws
            .create_folder(args.project.as_deref(), &args.path)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct DeleteEntityArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Path of the doc, file, or folder to delete.
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct DeleteEntityTool {
    ws: Arc<Workspace>,
}

impl Tool for DeleteEntityTool {
    type ARGUMENTS = DeleteEntityArgs;
    const NAME: &str = "delete_entity";
    const DESCRIPTION: Option<&str> = Some(prompts::DELETE_ENTITY);

    async fn invoke(&self, args: DeleteEntityArgs) -> Result<String, LLMYError> {
        self.ws
            .delete_entity(args.project.as_deref(), &args.path)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct RenameEntityArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Path of the doc, file, or folder to rename.
    pub path: String,
    /// New leaf name (no slashes).
    pub new_name: String,
}

#[derive(Clone, Debug)]
pub struct RenameEntityTool {
    ws: Arc<Workspace>,
}

impl Tool for RenameEntityTool {
    type ARGUMENTS = RenameEntityArgs;
    const NAME: &str = "rename_entity";
    const DESCRIPTION: Option<&str> = Some(prompts::RENAME_ENTITY);

    async fn invoke(&self, args: RenameEntityArgs) -> Result<String, LLMYError> {
        self.ws
            .rename_entity(args.project.as_deref(), &args.path, &args.new_name)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct MoveEntityArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Path of the doc, file, or folder to move.
    pub path: String,
    /// Destination folder path (created when missing); `/` is the project root.
    pub target_folder: String,
}

#[derive(Clone, Debug)]
pub struct MoveEntityTool {
    ws: Arc<Workspace>,
}

impl Tool for MoveEntityTool {
    type ARGUMENTS = MoveEntityArgs;
    const NAME: &str = "move_entity";
    const DESCRIPTION: Option<&str> = Some(prompts::MOVE_ENTITY);

    async fn invoke(&self, args: MoveEntityArgs) -> Result<String, LLMYError> {
        self.ws
            .move_entity(args.project.as_deref(), &args.path, &args.target_folder)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct UploadFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Local filesystem path to read.
    pub local_path: String,
    /// Destination path in the project, e.g. `/figures/plot.png`.
    pub project_path: String,
}

#[derive(Clone, Debug)]
pub struct UploadFileTool {
    ws: Arc<Workspace>,
}

impl Tool for UploadFileTool {
    type ARGUMENTS = UploadFileArgs;
    const NAME: &str = "upload_file";
    const DESCRIPTION: Option<&str> = Some(prompts::UPLOAD_FILE);

    async fn invoke(&self, args: UploadFileArgs) -> Result<String, LLMYError> {
        self.ws
            .upload_file(args.project.as_deref(), &args.local_path, &args.project_path)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct DownloadFileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Path of the doc or file in the project.
    pub path: String,
    /// Local filesystem path to write to.
    pub local_path: String,
}

#[derive(Clone, Debug)]
pub struct DownloadFileTool {
    ws: Arc<Workspace>,
}

impl Tool for DownloadFileTool {
    type ARGUMENTS = DownloadFileArgs;
    const NAME: &str = "download_file";
    const DESCRIPTION: Option<&str> = Some(prompts::DOWNLOAD_FILE);

    async fn invoke(&self, args: DownloadFileArgs) -> Result<String, LLMYError> {
        self.ws
            .download_file(args.project.as_deref(), &args.path, &args.local_path)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct GetHistoryArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Page further back: the timestamp offered by a previous get_history result.
    pub before: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct GetHistoryTool {
    ws: Arc<Workspace>,
}

impl Tool for GetHistoryTool {
    type ARGUMENTS = GetHistoryArgs;
    const NAME: &str = "get_history";
    const DESCRIPTION: Option<&str> = Some(prompts::GET_HISTORY);

    async fn invoke(&self, args: GetHistoryArgs) -> Result<String, LLMYError> {
        self.ws
            .get_history(args.project.as_deref(), args.before)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct LabelVersionArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// History version to label, as shown by get_history.
    pub version: i64,
    /// Label text, e.g. "submitted-draft".
    pub comment: String,
}

#[derive(Clone, Debug)]
pub struct LabelVersionTool {
    ws: Arc<Workspace>,
}

impl Tool for LabelVersionTool {
    type ARGUMENTS = LabelVersionArgs;
    const NAME: &str = "label_version";
    const DESCRIPTION: Option<&str> = Some(prompts::LABEL_VERSION);

    async fn invoke(&self, args: LabelVersionArgs) -> Result<String, LLMYError> {
        self.ws
            .label_version(args.project.as_deref(), args.version, &args.comment)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct CompileArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompileTool {
    ws: Arc<Workspace>,
}

impl Tool for CompileTool {
    type ARGUMENTS = CompileArgs;
    const NAME: &str = "compile";
    const DESCRIPTION: Option<&str> = Some(prompts::COMPILE);

    async fn invoke(&self, args: CompileArgs) -> Result<String, LLMYError> {
        self.ws.compile(args.project.as_deref()).await.into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadLogArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// 1-based line to start reading from (default: 1).
    pub offset: Option<usize>,
    /// Maximum number of lines to return (default: 200).
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ReadLogTool {
    ws: Arc<Workspace>,
}

impl Tool for ReadLogTool {
    type ARGUMENTS = ReadLogArgs;
    const NAME: &str = "read_log";
    const DESCRIPTION: Option<&str> = Some(prompts::READ_LOG);

    async fn invoke(&self, args: ReadLogArgs) -> Result<String, LLMYError> {
        self.ws
            .read_log(args.project.as_deref(), args.offset, args.limit)
            .await
            .into_llmy()
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct DownloadOutputArgs {
    /// Project name or id. Omit it when the server is restricted to a single
    /// project (see server instructions); any other project is then denied.
    pub project: Option<String>,
    /// Output file from the last compile (default: `output.pdf`).
    pub file: Option<String>,
    /// Local filesystem path to write to.
    pub local_path: String,
}

#[derive(Clone, Debug)]
pub struct DownloadOutputTool {
    ws: Arc<Workspace>,
}

impl Tool for DownloadOutputTool {
    type ARGUMENTS = DownloadOutputArgs;
    const NAME: &str = "download_output";
    const DESCRIPTION: Option<&str> = Some(prompts::DOWNLOAD_OUTPUT);

    async fn invoke(&self, args: DownloadOutputArgs) -> Result<String, LLMYError> {
        self.ws
            .download_output(
                args.project.as_deref(),
                args.file.as_deref().unwrap_or("output.pdf"),
                &args.local_path,
            )
            .await
            .into_llmy()
    }
}

impl Workspace {
    /// The complete MCP tool surface backed by this workspace.
    pub fn toolbox(self: &Arc<Self>) -> ToolBox {
        let mut tools = ToolBox::new();
        tools.add_tool(ListProjectsTool { ws: self.clone() });
        tools.add_tool(ListFilesTool { ws: self.clone() });
        tools.add_tool(ReadFileTool { ws: self.clone() });
        tools.add_tool(StatFileTool { ws: self.clone() });
        tools.add_tool(EditFileTool { ws: self.clone() });
        tools.add_tool(WriteFileTool { ws: self.clone() });
        tools.add_tool(SearchTool { ws: self.clone() });
        tools.add_tool(CreateFolderTool { ws: self.clone() });
        tools.add_tool(DeleteEntityTool { ws: self.clone() });
        tools.add_tool(RenameEntityTool { ws: self.clone() });
        tools.add_tool(MoveEntityTool { ws: self.clone() });
        tools.add_tool(UploadFileTool { ws: self.clone() });
        tools.add_tool(DownloadFileTool { ws: self.clone() });
        tools.add_tool(GetHistoryTool { ws: self.clone() });
        tools.add_tool(LabelVersionTool { ws: self.clone() });
        tools.add_tool(CompileTool { ws: self.clone() });
        tools.add_tool(ReadLogTool { ws: self.clone() });
        tools.add_tool(DownloadOutputTool { ws: self.clone() });
        tools
    }
}
