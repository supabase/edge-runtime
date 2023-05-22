#[macro_export]
macro_rules! integration_test {
    ($main_file:expr, $port:expr, $url:expr, $req:expr, $function: expr) => {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<base::server::ServerCodes>(1);

        let signal = tokio::spawn(async move {
            while let Some(base::server::ServerCodes::Listening) = rx.recv().await {
                if let Some(manual_req) = $req {
                    let exec = manual_req.send().await;
                    return Some(exec);
                } else {
                    let req = reqwest::get(format!("http://localhost:{}/{}", $port, $url)).await;
                    return Some(req);
                }
            }
            None
        });

        tokio::select! {
            resp = signal => {
                if let Ok(maybe_response_from_server) = resp {
                    $function(maybe_response_from_server.unwrap()).await;
                } else {
                    panic!("Request thread had a heart attack");
                }
            }
            _ = base::commands::start_server(
                "0.0.0.0",
                $port,
                String::from($main_file),
                None,
                false,
                Some(tx.clone())
            ) => {
                panic!("This one should not end first");
            }
        }
    };
}
