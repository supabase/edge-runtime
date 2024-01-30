#[cfg(target_os = "linux")]
use base::integration_test;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_not_trigger_pku_sigsegv_due_to_jit_compilation_cli() {
    integration_test!(
        "./test_cases/main",
        8999,
        "slow_resp",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            assert!(resp.unwrap().text().await.unwrap().starts_with("meow: "));
        })
    );
}
