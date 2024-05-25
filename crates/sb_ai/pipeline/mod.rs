pub mod auto_pipeline;
pub mod feature_extraction;

use anyhow::Error;
use log::error;
use ort::Session;
use std::path::PathBuf;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

use crate::session::create_session;

pub(crate) trait PipelineInput: Send + Sync {}
pub(crate) struct PipelineRequest<I: PipelineInput, R> {
    pub input: I,
    pub sender: UnboundedSender<Result<R, Error>>,
}

pub(crate) trait Pipeline<I: PipelineInput, R> {
    fn get_sender(&self) -> UnboundedSender<PipelineRequest<I, R>>;

    fn get_receiver(&self) -> Arc<Mutex<UnboundedReceiver<PipelineRequest<I, R>>>>;

    fn run(&self, session: &Session, tokenizer: &Tokenizer, input: &I) -> Result<R, Error>;

    async fn start_session(&self, model_dir: &PathBuf) {
        let req_rx = self.get_receiver();

        let session = create_session(model_dir.join("model.onnx"));
        let Ok(session) = session else {
            error!("sb_ai: failed to create session - {}", session.unwrap_err());
            return;
        };

        let tokenizer =
            Tokenizer::from_file(model_dir.join("tokenizer.json")).map_err(anyhow::Error::msg);
        let Ok(mut tokenizer) = tokenizer else {
            error!(
                "sb_ai: failed to create tokenizer - {}",
                tokenizer.unwrap_err()
            );
            return;
        };

        // TODO: move this responsability to pipeline
        // model's default max length is 128. Increase it to 512.
        let truncation = tokenizer.get_truncation_mut().unwrap();
        truncation.max_length = 512;

        loop {
            let req = req_rx.lock().await.recv().await;
            if req.is_none() {
                break;
            }
            let req = req.unwrap();

            let result = self.run(&session, &tokenizer, &req.input);
            if req.sender.send(result).is_err() {
                error!("sb_ai: failed to send inference results (channel error)");
            };
        }
    }
}
