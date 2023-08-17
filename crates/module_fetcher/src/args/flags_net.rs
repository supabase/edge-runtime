// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::anyhow::bail;
use deno_core::error::AnyError;
use deno_core::url::Url;
use std::net::IpAddr;
use std::str::FromStr;

#[derive(Debug, PartialEq, Eq)]
pub struct ParsePortError(String);

#[derive(Debug, PartialEq, Eq)]
pub struct BarePort(u16);

impl FromStr for BarePort {
    type Err = ParsePortError;
    fn from_str(s: &str) -> Result<BarePort, ParsePortError> {
        if s.starts_with(':') {
            match s.split_at(1).1.parse::<u16>() {
                Ok(port) => Ok(BarePort(port)),
                Err(e) => Err(ParsePortError(e.to_string())),
            }
        } else {
            Err(ParsePortError(
                "Bare Port doesn't start with ':'".to_string(),
            ))
        }
    }
}

pub fn validator(host_and_port: &str) -> Result<String, String> {
    if Url::parse(&format!("internal://{host_and_port}")).is_ok()
        || host_and_port.parse::<IpAddr>().is_ok()
        || host_and_port.parse::<BarePort>().is_ok()
    {
        Ok(host_and_port.to_string())
    } else {
        Err(format!("Bad host:port pair: {host_and_port}"))
    }
}

/// Expands "bare port" paths (eg. ":8080") into full paths with hosts. It
/// expands to such paths into 3 paths with following hosts: `0.0.0.0:port`,
/// `127.0.0.1:port` and `localhost:port`.
pub fn parse(paths: Vec<String>) -> Result<Vec<String>, AnyError> {
    let mut out: Vec<String> = vec![];
    for host_and_port in paths.iter() {
        if Url::parse(&format!("internal://{host_and_port}")).is_ok()
            || host_and_port.parse::<IpAddr>().is_ok()
        {
            out.push(host_and_port.to_owned())
        } else if let Ok(port) = host_and_port.parse::<BarePort>() {
            // we got bare port, let's add default hosts
            for host in ["0.0.0.0", "127.0.0.1", "localhost"].iter() {
                out.push(format!("{}:{}", host, port.0));
            }
        }
    }
    if out.is_empty() {
        bail!("Bad host:port pair")
    } else {
        Ok(out)
    }
}
