use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct Metadata {
    pub ca_stores: Option<Vec<String>>,
    pub ca_data: Option<Vec<u8>>,
    pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
}
