#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $policy:expr, $import_map:expr, $req_builder:expr, $tls:expr, ($($function:tt)+) $(, $termination_token:expr)?) => {
        use futures_util::FutureExt;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<base::server::ServerHealth>(1);

        let req_builder: Option<reqwest::RequestBuilder> = $req_builder;
        let tls: Option<base::server::Tls> = $tls;
        let signal = tokio::spawn(async move {
            while let Some(base::server::ServerHealth::Listening(event_rx)) = rx.recv().await {
                integration_test!(@req event_rx, $port, $url, req_builder, ($($function)+));
            }
            None
        });

        let termination_token = integration_test!(@term $(, $termination_token)?);
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
            termination_token.clone(),
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
        if let Some(token) = termination_token {
            let join_fut = tokio::spawn(async move {
                let _ = listen_fut.await;
            });

            let wait_fut = async move {
                token.cancel_and_wait().await;
                join_fut.await.unwrap();
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
        }
    };

    (@term , $termination_token:expr) => {
        Some($termination_token)
    };

    (@term) => {
        None
    };

    (@req $event_rx:ident, $port:expr, $url:expr, $req_builder:expr, ($req:expr, $_:expr)) => {
        return $req($port, $url, $req_builder, $event_rx).await;
    };

    (@req $_:ident, $port:expr, $url:expr, $req_builder:expr, $__:expr) => {
        if let Some(req_factory) = $req_builder {
            return Some(req_factory.send().await);
        } else {
            let req = reqwest::get(format!("http://localhost:{}/{}", $port, $url)).await;
            return Some(req);
        }
    };

    (@resp $var:ident, ($_:expr, $resp:expr)) => {
        $resp($var)
    };

    (@resp $var:ident, $resp:expr) => {
        $resp($var)
    };
}
