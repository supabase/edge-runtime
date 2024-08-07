// Below is roughly originated from eszip@0.60.0/src/v2.rs

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use eszip::{
    v2::{
        read_npm_section, EszipNpmPackageIndex, EszipV2Module, EszipV2Modules, EszipV2SourceSlot,
        HashedSection,
    },
    EszipV2, ModuleKind, ParseError,
};
use futures::{io::BufReader, AsyncRead, AsyncReadExt};
use hashlink::LinkedHashMap;

const ESZIP_V2_1_MAGIC: &[u8; 8] = b"ESZIP2.1";

pub async fn parse_v2_header<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<EszipV2, ParseError> {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic).await?;

    if !EszipV2::has_magic(&magic) {
        return Err(ParseError::InvalidV2);
    }

    let is_v3 = magic == *ESZIP_V2_1_MAGIC;
    let header = HashedSection::read(reader).await?;
    if !header.hash_valid() {
        return Err(ParseError::InvalidV2HeaderHash);
    }

    let mut modules = LinkedHashMap::<String, EszipV2Module>::new();
    let mut npm_specifiers = HashMap::new();

    let mut read = 0;

    // This macro reads n number of bytes from the header section. If the header
    // section is not long enough, this function will be early exited with an
    // error.
    macro_rules! read {
        ($n:expr, $err:expr) => {{
            if read + $n > header.len() {
                return Err(ParseError::InvalidV2Header($err));
            }
            let start = read;
            read += $n;
            &header.bytes()[start..read]
        }};
    }

    while read < header.len() {
        let specifier_len =
            u32::from_be_bytes(read!(4, "specifier len").try_into().unwrap()) as usize;
        let specifier = String::from_utf8(read!(specifier_len, "specifier").to_vec())
            .map_err(|_| ParseError::InvalidV2Specifier(read))?;

        let entry_kind = read!(1, "entry kind")[0];
        match entry_kind {
            0 => {
                let source_offset =
                    u32::from_be_bytes(read!(4, "source offset").try_into().unwrap());
                let source_len = u32::from_be_bytes(read!(4, "source len").try_into().unwrap());
                let source_map_offset =
                    u32::from_be_bytes(read!(4, "source map offset").try_into().unwrap());
                let source_map_len =
                    u32::from_be_bytes(read!(4, "source map len").try_into().unwrap());
                let kind = match read!(1, "module kind")[0] {
                    0 => ModuleKind::JavaScript,
                    1 => ModuleKind::Json,
                    2 => ModuleKind::Jsonc,
                    3 => ModuleKind::OpaqueData,
                    n => return Err(ParseError::InvalidV2ModuleKind(n, read)),
                };
                let source = if source_offset == 0 && source_len == 0 {
                    EszipV2SourceSlot::Ready(Arc::new([]))
                } else {
                    EszipV2SourceSlot::Pending {
                        offset: source_offset as usize,
                        length: source_len as usize,
                        wakers: vec![],
                    }
                };
                let source_map = if source_map_offset == 0 && source_map_len == 0 {
                    EszipV2SourceSlot::Ready(Arc::new([]))
                } else {
                    EszipV2SourceSlot::Pending {
                        offset: source_map_offset as usize,
                        length: source_map_len as usize,
                        wakers: vec![],
                    }
                };
                let module = EszipV2Module::Module {
                    kind,
                    source,
                    source_map,
                };
                modules.insert(specifier, module);
            }
            1 => {
                let target_len =
                    u32::from_be_bytes(read!(4, "target len").try_into().unwrap()) as usize;
                let target = String::from_utf8(read!(target_len, "target").to_vec())
                    .map_err(|_| ParseError::InvalidV2Specifier(read))?;
                modules.insert(specifier, EszipV2Module::Redirect { target });
            }
            2 if is_v3 => {
                // npm specifier
                let pkg_id = u32::from_be_bytes(read!(4, "npm package id").try_into().unwrap());
                npm_specifiers.insert(specifier, EszipNpmPackageIndex(pkg_id));
            }
            n => return Err(ParseError::InvalidV2EntryKind(n, read)),
        };
    }

    let npm_snapshot = if is_v3 {
        read_npm_section(reader, npm_specifiers).await?
    } else {
        None
    };

    Ok(EszipV2 {
        modules: EszipV2Modules(Arc::new(Mutex::new(modules))),
        npm_snapshot,
    })
}
