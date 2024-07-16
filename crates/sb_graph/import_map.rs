use anyhow::{anyhow, Error};
use deno_core::url::Url;
use import_map::{parse_from_json, ImportMap};
use std::fs;
use std::path::Path;
use urlencoding::decode;

pub fn load_import_map(maybe_path: Option<String>) -> Result<Option<ImportMap>, Error> {
    if let Some(path_str) = maybe_path {
        let json_str;
        let base_url;

        // check if the path is a data URI (prefixed with data:)
        // the data URI takes the following format
        // data:{encodeURIComponent(mport_map.json)?{encodeURIComponent(base_path)}
        if path_str.starts_with("data:") {
            let data_uri = Url::parse(&path_str)?;
            json_str = decode(data_uri.path())?.into_owned();
            base_url =
                Url::from_directory_path(decode(data_uri.query().unwrap_or(""))?.into_owned())
                    .map_err(|_| anyhow!("invalid import map base url"))?;
        } else {
            let path = Path::new(&path_str);
            let abs_path = std::env::current_dir().map(|p| p.join(path))?;
            json_str = fs::read_to_string(abs_path.clone())?;
            base_url = Url::from_directory_path(abs_path.parent().unwrap())
                .map_err(|_| anyhow!("invalid import map base url"))?;
        }

        let result = parse_from_json(base_url, json_str.as_str())?;
        Ok(Some(result.import_map))
    } else {
        Ok(None)
    }
}
