#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $policy:expr, $import_map:expr, $req_builder:expr, $tls:expr, ($($function:tt)+) $(, $($token:tt)+)?) => {
        use futures_util::FutureExt;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<base::server::ServerHealth>(1);

        let req_builder: Option<reqwest::RequestBuilder> = $req_builder;
        let tls: Option<base::server::Tls> = $tls;
        let schema = if tls.is_some() { "https" } else { "http" };
        let signal = tokio::spawn(async move {
            while let Some(base::server::ServerHealth::Listening(event_rx, metric_src)) = rx.recv().await {
                integration_test!(@req event_rx, metric_src, schema, $port, $url, req_builder, ($($function)+));
            }
            None
        });

        let token = integration_test!(@term $(, $($token)+)?);
        let mut listen_fut = base::commands::start_server(
            "0.0.0.0",
            $port,
            tls,
            String::from($main_file),
            None,
            None,
            $policy,
            $import_map,
            $crate::server::ServerFlags::default(),
            Some(tx.clone()),
            $crate::server::WorkerEntrypoints {
                main: None,
                events: None,
            },
            token.clone(),
            vec![],
            None
        )
        .boxed();

        async fn __req_fn<F, R>(
            input_fn: F,
            port: u16,
            url: &'static str,
            req_builder: Option<reqwest::RequestBuilder>,
            event_rx: mpsc::UnboundedReceiver<base::server::ServerEvent>,
            metric_src: sb_core::SharedMetricSource
        ) -> Option<Result<reqwest::Response, reqwest::Error>>
        where
            F: FnOnce(
                u16,
                &'static str,
                Option<reqwest::RequestBuilder>,
                mpsc::UnboundedReceiver<base::server::ServerEvent>,
                sb_core::SharedMetricSource
            ) -> R,
            R: std::future::Future<Output = Option<Result<reqwest::Response, reqwest::Error>>>,
        {
            input_fn(
                port,
                url,
                req_builder,
                event_rx,
                metric_src
            )
            .await
        }

        async fn __resp_fn <F, R>(
            input_fn: F,
            resp: Result<reqwest::Response, reqwest::Error>
        )
        where
            F: FnOnce(
                Result<reqwest::Response, reqwest::Error>
            ) -> R,
            R: std::future::Future<Output = ()>,
        {
            input_fn(resp).await;
        }

        tokio::select! {
            resp = signal => {
                if let Ok(maybe_response_from_server) = resp {
                    // then, after checking the response... (2)
                    let resp = maybe_response_from_server.unwrap();
                    integration_test!(@resp resp, ($($function)+)).await;
                } else {
                    panic!("Request thread had a heart attack");
                }
            }

            // we poll the listen future till get a response (1)
            _ = &mut listen_fut => {
                panic!("This one should not end first");
            }
        }

        // if we have a termination token, we should advence a listen future to
        // the end for a graceful exit. (3)
        if let Some(token) = token {
            let join_fut = tokio::spawn(async move {
                let _ = listen_fut.await;
            });

            integration_test!(@term_cleanup $($($token)+)?, token, join_fut);
        }
    };

    (@term , #[manual] $token:expr) => {
        Some($token)
    };

    (@term , $token:expr) => {
        Some($token)
    };


    (@term) => {
        None
    };

    (@term_cleanup $(#[manual] $_:expr)?, $__:ident, $___:ident) => {};
    (@term_cleanup $_:expr, $token:ident, $join_fut:ident) => {
        let wait_fut = async move {
            $token.cancel_and_wait().await;
            $join_fut.await.unwrap();
        };

        if tokio::time::timeout(
            // XXX(Nyannyacha): Should we apply variable timeout?
            core::time::Duration::from_secs(10),
            wait_fut
        )
        .await
        .is_err()
        {
            panic!("failed to terminate server within 10 seconds");
        }
    };

    (@req $event_rx:ident, $metric_src:ident, $schema:expr, $port:expr, $url:expr, $req_builder:expr, ($req:expr, $_:expr)) => {
        if let Some(resp) = __req_fn($req, $port, $url, $req_builder, $event_rx, $metric_src).await {
            return Some(resp);
        } else {
            let resp = reqwest::get(format!("{}://localhost:{}/{}", $schema, $port, $url)).await;
            return Some(resp);
        }
    };

    (@req $_:ident, $__:ident, $schema:expr, $port:expr, $url:expr, $req_builder:expr, $___:expr) => {
        if let Some(req_factory) = $req_builder {
            return Some(req_factory.send().await);
        } else {
            let resp = reqwest::get(format!("{}://localhost:{}/{}", $schema, $port, $url)).await;
            return Some(resp);
        }
    };

    (@resp $var:ident, ($_:expr, $resp:expr)) => {
        __resp_fn($resp, $var)
    };

    (@resp $var:ident, $resp:expr) => {
        __resp_fn($resp, $var)
    };
}
