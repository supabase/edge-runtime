use deno_core::error::{custom_error, generic_error, AnyError};
use deno_core::url::Url;
use deno_fs::OpenOptions;
use deno_permissions::NetDescriptor;
use std::borrow::Cow;
use std::path::Path;

pub struct Permissions {
    net_access_disabled: bool,
    allow_net: Option<Vec<NetDescriptor>>,
}

impl Default for Permissions {
    fn default() -> Self {
        Self::new(false, None)
    }
}

impl Permissions {
    pub fn new(net_access_disabled: bool, allow_net: Option<Vec<NetDescriptor>>) -> Self {
        Self {
            net_access_disabled,
            allow_net,
        }
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
    options = { net_access_disabled: bool, allow_net: Option<Vec<NetDescriptor>> },
    state = |state, options| {
        state.put::<Permissions>(Permissions::new(options.net_access_disabled, options.allow_net));
    }
);

impl deno_web::TimersPermission for Permissions {
    fn allow_hrtime(&mut self) -> bool {
        false
    }
}

impl deno_fetch::FetchPermissions for Permissions {
    fn check_net_url(&mut self, url: &Url, _api_name: &str) -> Result<(), AnyError> {
        if self.net_access_disabled {
            return Err(custom_error(
                "PermissionDenied",
                "net access disabled for the user worker",
            ));
        }

        if let Some(allow_net) = &self.allow_net {
            let hostname = url.host_str().ok_or(generic_error("empty host"))?.parse()?;
            let descriptor = NetDescriptor(hostname, url.port());
            if !allow_net.contains(&descriptor) {
                return Err(custom_error(
                    "PermissionDenied",
                    format!("Access to {descriptor} is not allowed for user worker"),
                ));
            }
        }

        Ok(())
    }

    fn check_read(&mut self, _p: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }
}

impl deno_net::NetPermissions for Permissions {
    fn check_net<T: AsRef<str>>(
        &mut self,
        host: &(T, Option<u16>),
        _api_name: &str,
    ) -> Result<(), AnyError> {
        if self.net_access_disabled {
            return Err(custom_error(
                "PermissionDenied",
                "net access disabled for the user worker",
            ));
        }

        if let Some(allow_net) = &self.allow_net {
            let hostname = host.0.as_ref().parse()?;
            let descriptor = NetDescriptor(hostname, host.1);
            if !allow_net.contains(&descriptor) {
                return Err(custom_error(
                    "PermissionDenied",
                    format!("Access to {descriptor} is not allowed for user worker"),
                ));
            }
        }
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
    fn check_net_url(&mut self, url: &Url, _api_name: &str) -> Result<(), AnyError> {
        if self.net_access_disabled {
            return Err(custom_error(
                "PermissionDenied",
                "net access disabled for the user worker",
            ));
        }
        if let Some(allow_net) = &self.allow_net {
            let hostname = url.host_str().ok_or(generic_error("empty host"))?.parse()?;
            let descriptor = NetDescriptor(hostname, url.port());
            if !allow_net.contains(&descriptor) {
                return Err(custom_error(
                    "PermissionDenied",
                    format!("Access to {descriptor} is not allowed for user worker"),
                ));
            }
        }
        Ok(())
    }
}

/// TODO: File system should be protected even if it's for main.
/// Some sort of permission before main is boostrapped should be put in place

impl deno_fs::FsPermissions for Permissions {
    fn check_open<'a>(
        &mut self,
        _resolved: bool,
        _read: bool,
        _write: bool,
        path: &'a Path,
        _api_name: &str,
    ) -> Result<Cow<'a, Path>, deno_io::fs::FsError> {
        Ok(Cow::Borrowed(path))
    }

    fn check_read(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read_all(&mut self, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read_blind(
        &mut self,
        _path: &Path,
        _display: &str,
        _api_name: &str,
    ) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write_partial(&mut self, _path: &Path, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write_all(&mut self, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write_blind(
        &mut self,
        _p: &Path,
        _display: &str,
        _api_name: &str,
    ) -> Result<(), AnyError> {
        Ok(())
    }

    fn check<'a>(
        &mut self,
        resolved: bool,
        open_options: &OpenOptions,
        path: &'a Path,
        api_name: &str,
    ) -> Result<Cow<'a, Path>, deno_io::fs::FsError> {
        self.check_open(
            resolved,
            open_options.read,
            open_options.write || open_options.append,
            path,
            api_name,
        )
    }
}

impl sb_node::NodePermissions for Permissions {
    fn check_net_url(&mut self, _url: &Url, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read(&mut self, _path: &Path) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_read_with_api_name(
        &mut self,
        _path: &Path,
        _api_name: Option<&str>,
    ) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_sys(&mut self, _kind: &str, _api_name: &str) -> Result<(), AnyError> {
        Ok(())
    }

    fn check_write_with_api_name(
        &mut self,
        _path: &Path,
        _api_name: Option<&str>,
    ) -> Result<(), AnyError> {
        Ok(())
    }
}
