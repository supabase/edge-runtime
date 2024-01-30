#[path = "../src/utils/integration_test_helper.rs"]
mod integration_test_helper;

use base::integration_test;

#[tokio::test]
async fn test_node_server() {
    let port = 8698;
    integration_test!(
        "./test_cases/node-server",
        port,
        "",
        None,
        None,
        None::<reqwest::RequestBuilder>,
        (|resp: Result<reqwest::Response, reqwest::Error>| async {
            let res = resp.unwrap();
            assert_eq!(res.status().as_u16(), 200);

            let body_bytes = res.bytes().await.unwrap();
            assert_eq!(
        body_bytes,
        "Look again at that dot. That's here. That's home. That's us. On it everyone you love, everyone you know, everyone you ever heard of, every human being who ever was, lived out their lives. The aggregate of our joy and suffering, thousands of confident religions, ideologies, and economic doctrines, every hunter and forager, every hero and coward, every creator and destroyer of civilization, every king and peasant, every young couple in love, every mother and father, hopeful child, inventor and explorer, every teacher of morals, every corrupt politician, every 'superstar,' every 'supreme leader,' every saint and sinner in the history of our species lived there-on a mote of dust suspended in a sunbeam."
    );
        })
    );
}
