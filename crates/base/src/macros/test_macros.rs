#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $function: expr) => {
        let (tx, mut rx) = mpsc::channel::<ServerCodes>(1);

        let signal = tokio::spawn(async move {
            while let Some(ServerCodes::Listening) = rx.recv().await {
                let req = reqwest::get(format!("http://localhost:{}/{}", $port, $url)).await;
                return Some(req);
            }
            None
        });

        select! {
            resp = signal => {
                if let Ok(maybe_response_from_server) = resp {
                    $function(maybe_response_from_server.unwrap()).await;
                } else {
                    panic!("Request thread had a heart attack");
                }
            }
            _ = start_server(
                "0.0.0.0",
                $port,
                String::from($main_file),
                None,
                None,
                false,
                Some(tx.clone())
            ) => {
                panic!("This one should not end first");
            }
        }
    };
}
