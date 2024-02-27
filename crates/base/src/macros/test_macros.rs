#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $shot_policy:expr, $import_map:expr, $req_builder:expr, ($($function:tt)+) $(, $termination_token: expr)?) => {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<base::server::ServerHealth>(1);

        let signal = tokio::spawn(async move {
            while let Some(base::server::ServerHealth::Listening(event_rx)) = rx.recv().await {
                integration_test!(@req event_rx, $port, $url, $req_builder, ($($function)+));
            }
            None
        });

        tokio::select! {
            resp = signal => {
                if let Ok(maybe_response_from_server) = resp {
                    let resp = maybe_response_from_server.unwrap();
                    integration_test!(@resp resp, ($($function)+)).await;
                } else {
                    panic!("Request thread had a heart attack");
                }
            }
            _ = base::commands::start_server(
                "0.0.0.0",
                $port,
                String::from($main_file),
                None,
                $shot_policy,
                $import_map,
                $crate::server::ServerFlags::default(),
                Some(tx.clone()),
                $crate::server::WorkerEntrypoints {
                    main: None,
                    events: None,
                },
                integration_test!(@term $(, $termination_token)?),
                None
            ) => {
                panic!("This one should not end first");
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
