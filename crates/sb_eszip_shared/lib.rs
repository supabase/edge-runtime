use eszip::Module;

pub static SUPABASE_ESZIP_VERSION: &[u8] = b"1.1";

pub static SUPABASE_ESZIP_VERSION_KEY: &str = "---SUPABASE-ESZIP-VERSION-ESZIP---";
pub static VFS_ESZIP_KEY: &str = "---SUPABASE-VFS-DATA-ESZIP---";
pub static SOURCE_CODE_ESZIP_KEY: &str = "---SUPABASE-SOURCE-CODE-ESZIP---";
pub static STATIC_FILES_ESZIP_KEY: &str = "---SUPABASE-STATIC-FILES-ESZIP---";

pub trait AsyncEszipDataRead: std::fmt::Debug + Send + Sync {
    fn ensure_module(&self, specifier: &str) -> Option<Module>;
    fn ensure_import_map(&self, specifier: &str) -> Option<Module>;
}
