// Keep this in sync with `deno()` above
pub fn deno() -> &'static str {
   concat!("Supa:","0")
}

pub fn get_user_agent() -> &'static str {
   concat!("Supa:","0")
}

pub fn is_canary() -> bool {
   false
}

pub fn release_version_or_canary_commit_hash() -> &'static str {
   concat!("Supa:","0")
}
