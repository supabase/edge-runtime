use std::path::{Path, PathBuf};

pub fn find_up<A, B>(filename: A, start: B) -> Option<PathBuf>
where
    A: AsRef<Path>,
    B: AsRef<Path>,
{
    let mut current = start.as_ref().to_path_buf();

    loop {
        if current.join(&filename).exists() {
            return Some(current.join(filename));
        }

        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    None
}
