// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use ring::digest::Context;
use ring::digest::SHA256;
pub fn gen(v: &[impl AsRef<[u8]>]) -> String {
    let mut ctx = Context::new(&SHA256);
    for src in v {
        ctx.update(src.as_ref());
    }
    faster_hex::hex_string(ctx.finish().as_ref())
}

#[cfg(test)]
mod tests {
    use crate::util::checksum::gen;

    #[test]
    fn test() {
        let input = vec![b"hello", b"world", b"hello", b"hello"];
        let output = gen(&input);
        assert_eq!(
            "00d03979f1c2c9cece94003e15ced43d1de8bf126f28d27b93f1e37874fb5395",
            output.as_str()
        );
    }
}
