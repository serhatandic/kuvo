# Files (Zed's project panel)

Shots: `shots/screens-tree-*`, `screens-file-*`. Code: `kuvo/src/panel/files.rs`; the box's `kuvo/folders`
with `files=1` and `kuvo/file`.

## What Zed does

Read: `crates/project_panel/src/project_panel.rs` (`expand_entry`, `collapse_entry`,
`toggle_expanded`, `render_entry`, its indent, how entries are sorted, auto-folded folders).
Behaviour only; nothing copied (GPL).

- A tree of the project, folders first, each level indented; a click folds or opens a folder, or
  opens a file in the editor. Folders' contents are scanned as they open.
- Version control's folders and what settings exclude are left out; ignored files show dimmed.

## What Kuvo takes

- The box lists one folder at a time as it opens (`kuvo/folders?files=1&hidden=1`): its folders,
  then its files, in the order the box gives, hidden ones too; the box leaves out `.git`, `.hg` and
  `.svn` (`files=1`). Open folders are read again as the tab shows again.
- A folder the box can't read says why, as the computer says it ("Permission denied"), with Try
  Again, in its files' place; the top folder unreadable fills the panel. An empty one says Empty.
- A file opens read-only in its own tab. The folder's files sit alongside it
  with a filter. Use the tree to browse folders or the filter to find a file.
  Code is highlighted and numbered, and supported Markdown is rendered.
  The tab says why a file is unavailable or too large.

The tree reads folders as they open, through `kuvo/folders`. The filter uses
the box's tree search. The conversation's tab strip keeps open files in reach.
