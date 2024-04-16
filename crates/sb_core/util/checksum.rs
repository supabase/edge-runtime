// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use ring::digest::Context;
use ring::digest::SHA256;
use std::fmt::Write;

pub fn gen(v: &[impl AsRef<[u8]>]) -> String {
    let mut ctx = Context::new(&SHA256);
    for src in v {
        ctx.update(src.as_ref());
    }
    let digest = ctx.finish();
    let mut hash_str = String::with_capacity(64);
    digest
        .as_ref()
        .iter()
        .for_each(|byte| write!(hash_str, "{byte:02x}")
            .expect("write! macro on string cannot fail"));

    hash_str
}

#[cfg(test)]
mod tests {
    use crate::util::checksum::gen;

    #[test]
    fn test() {
        let input = vec![b"hello", b"world", b"hello", b"hello"];
        let output = gen(&input);
        assert_eq!("00d03979f1c2c9cece94003e15ced43d1de8bf126f28d27b93f1e37874fb5395",output.as_str());
    }
}
