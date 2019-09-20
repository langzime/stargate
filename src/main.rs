use futures::TryFutureExt;
use std::env;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use hyper::{Client, Server, Body, Request, Response};
use hyper::service::{make_service_fn, service_fn};
use hyper_tls::HttpsConnector;
use log::{debug, info, warn, error};
use std::result::Result::{Ok, Err};
use location::ResolvedLocation;
use clap::{App, AppSettings};
use futures_util::future::join_all;
use tokio::fs;
use ansi_term::Color::{Green, Red, Yellow};

static EXAMPLES: &str = "EXAMPLES:";

#[macro_use]
pub mod errors;
mod location;
mod routes;
mod logging;
mod matcher;

use matcher::Matcher;
use errors::Error;
use routes::Route;

#[tokio::main]
async fn main() -> Result<(), Error>  {
    logging::init();
    debug!("Starting");
    run().await?;
    Ok(())
}

async fn run() -> Result<(), Error> {
    let (routes, other_args) = routes::from_args(env::args().skip(1)).map_err(|e| {
        err!("failed to parse routes: {}", e)
    })?;
    let _ = App::new("weave")
        .author("James Wilson <james@jsdw.me>")
        .about("A lightweight HTTP router and file server.")
        .version("0.2")
        .after_help(EXAMPLES)
        .usage("weave SOURCE to DEST [and SOURCE to DEST ...]")
        .setting(AppSettings::NoBinaryName)
        .get_matches_from(other_args);

    if routes.is_empty() {
        return Err(err!("No routes have been provided. Use -h or --help for more information"));
    }

    // Log our routes:
    for route in &routes {
        info!("Routing {} to {}", route.src, route.dest);
    }

    // Partition provided routes based on the SocketAddr we'll serve them on:
    let mut map = HashMap::new();
    for route in routes {
        let socket_addr = route.src_socket_addr()?;
        let rs: &mut Vec<Route> = map.entry(socket_addr).or_default();
        rs.push(route);
    }

    let mut vec = Vec::new();
    for (socket_addr, routes) in map {
        let handler = handle_requests(socket_addr, routes);
        vec.push(handler);
    }
    join_all(vec).await;
    Result::<_, Error>::Ok(())
}

/// Handle incoming requests by matching on routes and dispatching as necessary
async fn handle_requests(socket_addr: SocketAddr, routes: Vec<Route>) {
    let socket_addr_outer = socket_addr.clone();

    let matcher = Arc::new(Matcher::new(routes));
    let socket_addr = Arc::new(socket_addr);

    let make_svc = make_service_fn(move |_| {
        let socket_addr = Arc::clone(&socket_addr);
        let matcher = Arc::clone(&matcher);
        async {
            Ok::<_, Error>(service_fn(move |_req| {
                let socket_addr = Arc::clone(&socket_addr);
                let matcher = Arc::clone(&matcher);
                async {
                    Ok::<_, Error>(handle_request(_req, socket_addr, matcher).await)
                }
            }))
        }
    });

    let server = Server::bind(&socket_addr_outer)
        .serve(make_svc);

    if let Err(e) = server.await {
        error!("{}", e);
    }
}

/// Handle a single request, given a matcher that defines how to map from input to output:
async fn handle_request<'a>(req: Request<Body>, socket_addr: Arc<SocketAddr>, matcher: Arc<Matcher>) -> Response<Body> {
    let before_time = std::time::Instant::now();
    let src_path = format!("{}{}", socket_addr, req.uri());
    let dest_path = matcher.resolve(req.uri());

    match dest_path {
        None => {
            let duration = before_time.elapsed();
            let not_found_string = format!("[no matching routes] {} in {:#?}", src_path, duration);
            warn!("{}", Red.paint(not_found_string));
            Response::builder()
                .status(404)
                .body(Body::from("Weave: No routes matched"))
                .unwrap()
        }
        Some(dest_path) => {
            match do_handle_request(req, &dest_path).await {
                Ok(resp) => {
                    let duration = before_time.elapsed();
                    let status_code = resp.status().as_u16();
                    let status_col =
                        if status_code >= 200 && status_code < 300 { Green } else if status_code >= 300 && status_code < 400 { Yellow } else { Red };

                    let info_string = format!("[{}] {} to {} in {:#?}",
                                              resp.status().as_str(),
                                              src_path,
                                              dest_path.to_string(),
                                              duration);
                    info!("{}", status_col.paint(info_string));
                    resp
                }
                Err(err) => {
                    let duration = before_time.elapsed();
                    let error_string = format!("[500] {} to {} ({}) in {:#?}",
                                               src_path,
                                               dest_path.to_string(),
                                               err,
                                               duration);
                    warn!("{}", Red.paint(error_string));
                    Response::builder()
                        .status(500)
                        .body(Body::from(format!("Weave: {}", err)))
                        .unwrap()
                }
            }
        }
    }
}

async fn do_handle_request(mut req: Request<Body>, dest_path: &ResolvedLocation) -> Result<Response<Body>, Error> {
    match dest_path {
        // Proxy to the URI our request matched against:
        ResolvedLocation::Url(url) => {
            // Set the request URI to our new destination:
            *req.uri_mut() = format!("{}", url).parse().unwrap();
            // Remove the host header (it's set according to URI if not present):
            req.headers_mut().remove("host");
            // Supoprt HTTPS (8 DNS worker threads):
            let https = HttpsConnector::new()?;
            // Proxy the request through and pass back the response:
            let response = Client::builder()
                .build(https)
                .request(req)
                .await?;
            Ok(response)
        }
        // Proxy to the filesystem:
        ResolvedLocation::FilePath(path) => {
            let mut file = Err(err!("File not found"));
            let mut mime = None;

            for end in &["", "index.htm", "index.html"] {
                let mut p = path.clone();
                if !end.is_empty() { p.push(end) }
                mime = Some(mime_guess::from_path(&p).first_or_octet_stream());
                file = fs::read(p).map_err(|e| err!("{}", e)).await;
                if file.is_ok() { break; }
            }

            let response = match file {
                Ok(file) => {
                    Response::builder()
                        .status(200)
                        .header("Content-Type", mime.unwrap().as_ref())
                        .body(Body::from(file))
                        .unwrap()
                }
                Err(e) => {
                    let msg = format!("Weave: Could not read file '{}': {}", path.to_string_lossy(), e);
                    Response::builder()
                        .status(404)
                        .body(Body::from(msg))
                        .unwrap()
                }
            };
            Ok(response)
        }
    }
}