use std::path::Path;
use std::path::PathBuf;

pub fn get_atomic_file_path(file_path: &Path) -> PathBuf {
  let rand = gen_rand_path_component();
  let extension = format!("{rand}.tmp");
  file_path.with_extension(extension)
}

fn gen_rand_path_component() -> String {
  (0..4).fold(String::with_capacity(8), |mut output, _| {
    write!(&mut output, "{:02x}", rand::random::<u8>()).unwrap();
    output
  })
}
