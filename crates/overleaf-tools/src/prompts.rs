//! All model-facing tool descriptions live here so the wording can be reviewed
//! and tuned in one place.

pub const LIST_PROJECTS: &str = "List the Overleaf projects the account can access, with their ids and names. Other tools accept either the project name or the id. When the server is restricted to a single project, only that project is listed and it is the only one any tool may touch.";

pub const LIST_FILES: &str = "List all files and folders in an Overleaf project. Editable text files are marked as `doc`, binary assets as `file`. The compile root document is marked when known.";

pub const READ_FILE: &str = "Read a text file (doc) from an Overleaf project, returned with line numbers in `cat -n` style. Reads a specific line range when asked: `offset` is the 1-based line to start from and `limit` the number of lines (e.g. offset 100, limit 50 returns lines 100-149); omit both for the whole file. Use stat_file first to learn how many lines a file has. You must read a file before you can edit or overwrite it.";

pub const EDIT_FILE: &str = "Replace an exact string in an Overleaf doc with new text, applied as a live collaborative edit (other people's concurrent edits are preserved). `old_string` must match the current content exactly and, unless `replace_all` is set, must be unique in the file. Requires a prior read_file of the same file in this session; if the content changed remotely and `old_string` no longer matches, re-read the file and retry. When the result notes that other collaborators edited the file, re-read it before further edits; if that keeps happening, people are actively working on the document — consider pausing your edits and telling the user instead of contending. Characters outside the Unicode BMP (e.g. emoji) cannot be stored by Overleaf and are rejected. After finishing a round of edits, run compile to verify the project still builds.";

pub const WRITE_FILE: &str = "Create a new doc or overwrite an existing one with the given full content. Missing parent folders are created. Overwriting requires that the file was read since its last change (read_file first). For binary assets use upload_file instead. Characters outside the Unicode BMP (e.g. emoji) cannot be stored by Overleaf and are rejected. After finishing a round of edits, run compile to verify the project still builds.";

pub const STAT_FILE: &str = "Get metadata about a project entry without reading its content: line count, byte size, and version for docs; size for binary files; entry counts for folders. Does not count as reading the file for edit purposes.";

pub const SEARCH: &str = "Search all docs of an Overleaf project with a regular expression, returning `path:line: text` matches. Restrict the scope by passing `path` (a doc path or a folder path). Binary files are not searched. Tip: to get an outline of a document's structure, search for LaTeX sectioning commands, e.g. pattern `\\\\(part|chapter|section|subsection|subsubsection)\\*?\\{` — the resulting `path:line:` list maps headings to line numbers, ready for targeted read_file offset/limit ranges.";

pub const CREATE_FOLDER: &str = "Create a folder (and any missing parents) in an Overleaf project.";

pub const DELETE_ENTITY: &str = "Permanently delete a doc, file, or folder (recursively) from an Overleaf project.";

pub const RENAME_ENTITY: &str = "Rename a doc, file, or folder in place. `new_name` is the new leaf name, not a path; use move_entity to relocate.";

pub const MOVE_ENTITY: &str = "Move a doc, file, or folder into another folder of the same project. The target folder is created when missing.";

pub const UPLOAD_FILE: &str = "Upload a local file into an Overleaf project at `project_path`. An existing entry with the same name is replaced (upsert). Text files with recognized extensions become editable docs; other files are stored as binary assets.";

pub const DOWNLOAD_FILE: &str = "Download a project doc or binary file to a local path.";

pub const GET_HISTORY: &str = "Show the project's edit history, newest first: version ranges, when and by whom, which files were edited, structural changes (add/remove/rename), and existing labels. History versions are project-wide and are NOT the per-doc versions reported by read_file/edit_file. Pass `before` (the timestamp offered by a previous result) to page further back.";

pub const LABEL_VERSION: &str = "Attach a named label to a project history version so it can be found and restored later in the Overleaf history UI. `version` must be a history version as shown by get_history, not a doc version.";

pub const COMPILE: &str = "Compile the Overleaf project with its configured LaTeX engine and return the compile status plus errors and warnings parsed from the log (a summary, never the full log). When the summary is not enough to diagnose a failure, page through the complete log with read_log; use download_output to save output.pdf or other artifacts.";

pub const READ_LOG: &str = "Read the full output.log of the most recent compile of this session, with line numbers. Returns at most `limit` lines per call (default 200) starting at 1-based line `offset`; the header shows the total line count for paging. Run compile first.";

pub const DOWNLOAD_OUTPUT: &str = "Download an output file (e.g. output.pdf or output.log) produced by the most recent compile of this session to a local path. Run compile first.";
