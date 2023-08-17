use anyhow::Error;
use deno_core::futures::io::{AllowStdIo, BufReader};
use deno_core::serde_json;
use deno_core::url::Url;
use deno_core::{op, ByteString, JsBuffer, OpState};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

deno_core::extension!(sb_eszip, ops = [op_eszip_extract], esm = ["eszip.js"]);

fn module_path_from_specifier(specifier: &str) -> Result<PathBuf, Error> {
    let specifier = Url::parse(specifier)?;
    let hostname = specifier.host_str().unwrap_or("");
    let path_components: Vec<&str> = specifier
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let mut module_path = PathBuf::new();
    module_path.push(".");
    module_path.push(hostname);
    for component in path_components {
        module_path.push(component);
    }

    // Some paths may not have an extension set,
    // or have version intead of an extension
    // Set extenson explicitly for such paths.
    // eg: https://esm.sh/react
    // eg: https://esm.sh/react@18.2.0
    let extension = module_path
        .extension()
        .map(|s| s.to_str().unwrap())
        .unwrap_or("");
    if extension.is_empty() || extension.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(module_path.with_extension("js"));
    }

    Ok(module_path)
}

fn write(p: PathBuf, source: String) -> Result<(), std::io::Error> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
        return std::fs::write(p, source);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "destination directory is missing",
    ))
}

#[derive(Serialize, Deserialize)]
struct ImportMap {
    imports: HashMap<String, PathBuf>,
}

#[op]
pub async fn op_eszip_extract(
    _state: Rc<RefCell<OpState>>,
    bytes: JsBuffer,
    dest_path: ByteString,
) -> Result<(), Error> {
    let dest_path = Path::new(std::str::from_utf8(&dest_path)?);
    // TODO: do we need to convert to a vec?
    let bytes = Vec::from(&*bytes);

    let bufreader = BufReader::new(AllowStdIo::new(bytes.as_slice()));
    let (eszip, loader) = eszip::EszipV2::parse(bufreader).await?;

    let fut = async move {
        let mut imports = HashMap::new();
        let specifiers = eszip.specifiers();
        for specifier in specifiers {
            if let Some(module) = eszip.get_module(&specifier) {
                if let Some(source) = module.source().await {
                    let source = std::str::from_utf8(&source)?;
                    // write module to disk
                    let module_path = module_path_from_specifier(&module.specifier)?;
                    write(dest_path.join(&module_path), source.to_string())?;

                    // track import
                    imports.insert(specifier.clone(), module_path.clone());
                    if specifier != module.specifier {
                        imports.insert(module.specifier, module_path.clone());
                    }
                }
            }
        }

        // write import map
        let import_map = ImportMap { imports };
        write(
            dest_path.join("import_map.json"),
            serde_json::to_string(&import_map)?,
        )?;

        Ok::<(), Error>(())
    };

    tokio::try_join!(
        async move {
            loader.await?;
            Ok::<(), Error>(())
        },
        fut
    )?;

    Ok(())
}
