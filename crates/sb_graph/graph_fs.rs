use deno_core::ModuleSpecifier;
use eszip::deno_graph;

pub struct DenoGraphFsAdapter<'a>(pub &'a dyn deno_fs::FileSystem);

impl<'a> deno_graph::source::FileSystem for DenoGraphFsAdapter<'a> {
    fn read_dir(&self, dir_url: &deno_graph::ModuleSpecifier) -> Vec<deno_graph::source::DirEntry> {
        use deno_graph::source::DirEntry;
        use deno_graph::source::DirEntryKind;

        let dir_path = match dir_url.to_file_path() {
            Ok(path) => path,
            // ignore, treat as non-analyzable
            Err(()) => return vec![],
        };
        let entries = match self.0.read_dir_sync(&dir_path) {
            Ok(dir) => dir,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                return vec![];
            }
            Err(err) => {
                return vec![DirEntry {
                    kind: DirEntryKind::Error(
                        anyhow::Error::from(err).context("Failed to read directory.".to_string()),
                    ),
                    url: dir_url.clone(),
                }];
            }
        };
        let mut dir_entries = Vec::with_capacity(entries.len());
        for entry in entries {
            let entry_path = dir_path.join(&entry.name);
            dir_entries.push(if entry.is_directory {
                DirEntry {
                    kind: DirEntryKind::Dir,
                    url: ModuleSpecifier::from_directory_path(&entry_path).unwrap(),
                }
            } else if entry.is_file {
                DirEntry {
                    kind: DirEntryKind::File,
                    url: ModuleSpecifier::from_file_path(&entry_path).unwrap(),
                }
            } else if entry.is_symlink {
                DirEntry {
                    kind: DirEntryKind::Symlink,
                    url: ModuleSpecifier::from_file_path(&entry_path).unwrap(),
                }
            } else {
                continue;
            });
        }

        dir_entries
    }
}
