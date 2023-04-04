use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::Extension;
use std::path::Path;

pub struct Permissions;

impl Permissions {
    pub fn new() -> Self {
        Self
    }

    pub fn check_env(&mut self, _var: &str) -> Result<(), AnyError> {
        Ok(())
    }

    pub fn check_env_all(&mut self) -> Result<(), AnyError> {
        Ok(())
    }

    pub fn check_read_blind(
        &mut self,
        _path: &Path,
        _display: &str,
        _api_name: &str,
    ) -> Result<(), AnyError> {
        Ok(())
    }
}

pub fn init() -> Extension {
    Extension::builder("permissions")
        .state(move |state| {
            state.put::<Permissions>(Permissions::new());
            ()
        })
        .build()
}

impl deno_web::TimersPermission for Permissions {
    fn allow_hrtime(&mut self) -> bool {
        true
    }

    fn check_unstable(&self, _state: &deno_core::OpState, _api_name: &'static str) {}
}

impl deno_fetch::FetchPermissions for Permissions {
    fn check_net_url(&mut self, _url: &Url, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read(&mut self, _p: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }
}

impl deno_net::NetPermissions for Permissions {
    fn check_net<T: AsRef<str>>(
        &mut self,
        _host: &(T, Option<u16>),
        _api_name: &str,
    ) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }
}

impl deno_websocket::WebSocketPermissions for Permissions {
    fn check_net_url(&mut self, _url: &Url, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }
}
