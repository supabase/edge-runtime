use anyhow::Error;
use deno_core::url::Url;
use deno_core::JsRuntime;
use deno_core::RuntimeOptions;
use log::debug;
use std::fs;
use std::panic;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub mod http_start;
pub mod module_loader;
pub mod net_override;
pub mod permissions;

use module_loader::DefaultModuleLoader;
use permissions::Permissions;

pub struct JsWorker {
    service_path: PathBuf,
}

impl JsWorker {
    pub fn new(service_path: PathBuf) -> Self {
        Self { service_path }
    }

    fn start_runtime(
        &self,
        tcp_stream_rx: mpsc::UnboundedReceiver<TcpStream>,
        shutdown_tx: oneshot::Sender<()>,
    ) {
        let user_agent = "rex".to_string();
        let extensions_with_js = vec![
            deno_webidl::init(),
            deno_console::init(),
            deno_url::init(),
            deno_web::init::<Permissions>(deno_web::BlobStore::default(), None),
            deno_fetch::init::<Permissions>(deno_fetch::Options {
                // TODO: update user agent
                user_agent: user_agent.clone(),
                ..Default::default()
            }),
            // TODO: support providing a custom seed for crypto
            deno_crypto::init(None),
            // TODO: configure root cert store
            deno_net::init::<Permissions>(None, false, None),
            deno_websocket::init::<Permissions>(user_agent.clone(), None, None),
            deno_http::init(),
        ];
        let extensions = vec![
            net_override::init(),
            http_start::init(),
            permissions::init(),
        ];

        let mut js_runtime = JsRuntime::new(RuntimeOptions {
            extensions,
            extensions_with_js,
            module_loader: Some(Rc::new(DefaultModuleLoader)),
            is_main: true,
            ..Default::default()
        });

        // bootstrap the JS runtime
        let bootstrap_js = include_str!("./js_worker/js/bootstrap.js");
        js_runtime
            .execute_script("[js_worker]: bootstrap.js", bootstrap_js)
            .unwrap();

        //run inside a closure, so op_state_rc is released
        {
            let op_state_rc = js_runtime.op_state();
            let mut op_state = op_state_rc.borrow_mut();
            op_state.put::<mpsc::UnboundedReceiver<TcpStream>>(tcp_stream_rx);
        }

        let module_path = self.service_path.join("index.ts");
        // TODO: handle file missing error
        let main_module_code = fs::read_to_string(&module_path).unwrap();
        let main_module_url = Url::from_file_path(
            std::env::current_dir()
                .map(|p| p.join(&module_path))
                .unwrap(),
        )
        .unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let future = async move {
            let mod_id = js_runtime
                .load_main_module(&main_module_url, Some(main_module_code))
                .await?;
            let result = js_runtime.mod_evaluate(mod_id);
            js_runtime.run_event_loop(false).await?;
            result.await?
        };
        let local = tokio::task::LocalSet::new();
        let res = local.block_on(&runtime, future);

        if res.is_err() {
            panic!("{:?}", res.as_ref().err().unwrap());
        }
        shutdown_tx
            .send(())
            .expect("failed to send shutdown signal");
    }

    pub async fn serve(service_path: PathBuf, tcp_stream: TcpStream) -> Result<(), Error> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (tcp_stream_tx, tcp_stream_rx) = mpsc::unbounded_channel::<TcpStream>();

        let _ = thread::spawn(move || {
            let js_worker = Self::new(service_path);
            js_worker.start_runtime(tcp_stream_rx, shutdown_tx)
        });

        tcp_stream_tx.send(tcp_stream)?;

        debug!("js worker thread started");

        // wait for shutdown signal
        let _ = shutdown_rx.await;

        Ok(())
    }
}
