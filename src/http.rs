use std::io::Read;
use std::time::Duration;

use ureq::http::Response;
use ureq::{Agent, Body};

use crate::cancel::Cancel;
use crate::error::{Error, Result};

pub fn agent(timeout: Duration, follow_redirects: bool) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(if follow_redirects { 10 } else { 0 })
        .max_redirects_will_error(false)
        .user_agent(concat!("veo/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub fn get(url: &str, timeout: Duration, headers: &[(&str, &str)]) -> Result<Response<Body>> {
    let mut request = agent(timeout, true).get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    Ok(request.call()?)
}

pub fn head(url: &str, timeout: Duration) -> Result<Response<Body>> {
    Ok(agent(timeout, true).head(url).call()?)
}

pub fn text(response: &mut Response<Body>, limit: u64) -> Result<String> {
    Ok(response.body_mut().with_config().limit(limit).read_to_string()?)
}

pub fn header<'a>(response: &'a Response<Body>, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|value| value.to_str().ok())
}

/// Stream the body in chunks, stopping promptly when cancelled.
pub fn stream(response: &mut Response<Body>, cancel: &Cancel, mut each: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        cancel.check()?;
        let read = reader.read(&mut buffer).map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                Error::timeout("request timed out")
            } else {
                Error::from(error)
            }
        })?;
        if read == 0 {
            return Ok(());
        }
        each(&buffer[..read])?;
    }
}
