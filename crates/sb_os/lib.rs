use std::collections::HashMap;

pub type EnvVars = HashMap<String, String>;

deno_core::extension!(sb_os, esm_entry_point = "ext:sb_os/os.js", esm = ["os.js"]);
