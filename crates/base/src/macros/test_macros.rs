#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $policy:expr, $import_map:expr, $req_builder:expr, $tls:expr, ($($function:tt)+) $(, $($token:tt)+)?) => {
        use futures_util::FutureExt;
        use $crate::macros::test_macros::__private;

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
        if let Some(resp) = __private::infer_req_closure_signature(
            $req,
            (
                $port,
                $url,
                $req_builder,
                $event_rx,
                $metric_src,
            )
        )
        .await
        {
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
        __private::infer_resp_closure_signature($resp, $var)
    };

    (@resp $var:ident, $resp:expr) => {
        __private::infer_resp_closure_signature($resp, $var)
    };
}

#[doc(hidden)]
pub mod __private {
    use std::future::Future;

    use reqwest::{Error, RequestBuilder, Response};
    use sb_core::SharedMetricSource;
    use tokio::sync::mpsc;

    use crate::server::ServerEvent;

    /// NOTE(Nyannyacha): This was defined to enable pattern matching in closure
    /// argument positions.
    type ReqTuple = (
        u16,
        &'static str,
        Option<RequestBuilder>,
        mpsc::UnboundedReceiver<ServerEvent>,
        SharedMetricSource,
    );

    pub async fn infer_req_closure_signature<F, R>(
        closure: F,
        args: ReqTuple,
    ) -> Option<Result<Response, Error>>
    where
        F: FnOnce(ReqTuple) -> R,
        R: Future<Output = Option<Result<Response, Error>>>,
    {
        closure(args).await
    }

    pub async fn infer_resp_closure_signature<F, R>(closure: F, arg0: Result<Response, Error>)
    where
        F: FnOnce(Result<Response, Error>) -> R,
        R: Future<Output = ()>,
    {
        closure(arg0).await;
    }
}
