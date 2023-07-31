use std::collections::HashMap;

pub type EnvVars = HashMap<String, String>;

deno_core::extension!(sb_os, esm = ["os.js"]);
