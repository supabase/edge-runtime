use std::sync::Arc;

use anyhow::Result;
use deno_core::{serde_v8::to_v8, ToV8};
use ort::Session;
use reqwest::Url;

use super::session::{get_session, load_session_from_bytes, load_session_from_url};

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub input_names: Vec<String>,
    pub output_names: Vec<String>,
}

#[derive(Debug)]
pub struct ModelSession {
    info: ModelInfo,
    inner: Arc<Session>,
}

impl ModelSession {
    fn new(id: String, session: Arc<Session>) -> Self {
        let input_names = session
            .inputs
            .iter()
            .map(|input| input.name.to_owned())
            .collect::<Vec<_>>();

        let output_names = session
            .outputs
            .iter()
            .map(|output| output.name.to_owned())
            .collect::<Vec<_>>();

        Self {
            info: ModelInfo {
                id,
                input_names,
                output_names,
            },
            inner: session,
        }
    }

    pub fn info(&self) -> ModelInfo {
        self.info.to_owned()
    }

    pub fn inner(&self) -> Arc<Session> {
        self.inner.clone()
    }

    pub fn from_id(id: String) -> Option<Self> {
        get_session(&id).map(|session| Self::new(id, session))
    }

    pub async fn from_url(model_url: Url) -> Result<Self> {
        load_session_from_url(model_url)
            .await
            .map(|(id, session)| Self::new(id, session))
    }

    pub fn from_bytes(model_bytes: &[u8]) -> Result<Self> {
        load_session_from_bytes(model_bytes).map(|(id, session)| Self::new(id, session))
    }
}

impl<'a> ToV8<'a> for ModelInfo {
    type Error = std::convert::Infallible;

    fn to_v8(
        self,
        scope: &mut deno_core::v8::HandleScope<'a>,
    ) -> std::result::Result<deno_core::v8::Local<'a, deno_core::v8::Value>, Self::Error> {
        let v8_values = to_v8(scope, (self.id, self.input_names, self.output_names));

        Ok(v8_values.unwrap())
    }
}
