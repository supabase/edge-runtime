use deno_core::error::AnyError;
use deno_core::url::Url;
use std::path::Path;

pub struct Permissions;

impl Default for Permissions {
    fn default() -> Self {
        Self::new()
    }
}

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

deno_core::extension!(
    sb_core_permissions,
    state = |state| {
        state.put::<Permissions>(Permissions::new());
    }
);

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
