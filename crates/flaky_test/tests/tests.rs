use flaky_test::flaky_test;
async fn str_len_async(s: &str) -> usize {
    // do something awaitable ideally...
    s.len()
}

#[flaky_test]
async fn fail_first_two_times() {
    println!("{}", str_len_async("Hello World").await);
}
