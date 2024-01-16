#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, ($($function:tt)+) $(, $termination_token: expr)?) => {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<base::server::ServerCodes>(1);

        let signal = tokio::spawn(async move {
            while let Some(base::server::ServerCodes::Listening) = rx.recv().await {
                integration_test!(@req $port, $url, ($($function)+));
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
                None,
                None,
                false,
                Some(tx.clone()),
                $crate::server::WorkerEntrypoints {
                    main: None,
                    events: None,
                },
                integration_test!(@term $(, $termination_token)?)
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

    (@req $port:expr, $url:expr, ($req:expr, $_:expr)) => {
        return $req($port, $url).await;
    };

    (@req $port:expr, $url:expr, $_:expr) => {
        let req = reqwest::get(format!("http://localhost:{}/{}", $port, $url)).await;
        return Some(req);
    };

    (@resp $var:ident, ($_:expr, $resp:expr)) => {
        $resp($var)
    };

    (@resp $var:ident, $resp:expr) => {
        $resp($var)
    };
}
