use anyhow::Error;
use log::info;

pub struct Server {
    host: String,
    port: u16,
    services_dir: String,
}

impl Server {
    pub fn new(host: String, port: u16, services_dir: String) -> Self {
        Self {
            host,
            port,
            services_dir,
        }
    }

    pub fn listen(&self) -> Result<(), Error> {
        // start js workers (for each service)
        // start tcp listener
        info!("listening on {}:{:?}", self.host, self.port);
        Ok(())
    }
}
