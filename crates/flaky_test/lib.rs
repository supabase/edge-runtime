extern crate proc_macro;
extern crate syn;
use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_attribute]
pub fn flaky_test(_attr: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = syn::parse_macro_input!(input as syn::ItemFn);
    let name = input_fn.sig.ident.clone();

    TokenStream::from(quote! {
      #[tokio::test]
      async fn #name() {
        #input_fn
        for i in 0..3 {
            let res = tokio::task::spawn_blocking(|| {
                let catch_unwind = std::panic::catch_unwind(|| {
                    let handle = tokio::runtime::Handle::current();
                        let _ = handle.enter();
                        handle.block_on(#name());
                });
                catch_unwind
            }).await;

            if let Ok(thread_res) = res {
                if thread_res.is_ok() { return; }
            }

            std::thread::sleep(std::time::Duration::from_secs_f32(1.25))
        }
        panic!("Test finished but did not ok");
      }
    })
}
