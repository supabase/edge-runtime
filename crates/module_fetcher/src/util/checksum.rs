// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use ring::digest::Context;
use ring::digest::SHA256;

pub fn gen(v: &[impl AsRef<[u8]>]) -> String {
    let mut ctx = Context::new(&SHA256);
    for src in v {
        ctx.update(src.as_ref());
    }
    let digest = ctx.finish();
    let out: Vec<String> = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    out.join("")
}
