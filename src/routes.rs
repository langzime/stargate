use std::net::{ SocketAddr, ToSocketAddrs };
use crate::errors::{ Error };
use crate::location::{ SrcLocation, DestLocation };

/// Take some args and hand back a vector of Routes we've parsed out of them,
/// plus an Iterator of unused args:
pub fn from_args<I: IntoIterator<Item=String>>(args: I) -> Result<(Vec<Route>, impl Iterator<Item=String>), Error> {
    let mut routes = vec![];

    // If the first arg is a Location, expect the next two args to be
    // 'to' and another Location. Each time the subsequent arg is 'and',
    // look for the same again.
    let mut args = args.into_iter().peekable();
    let mut expects_more = false;
    while let Some(peeked) = args.peek() {
        let peeked = peeked.clone();
        if let Ok(src) = SrcLocation::parse(&peeked) {

            // we've parsed more:
            expects_more = false;

            // Next arg is valid Location (we peeked), so assume
            // 'loc to loc' triplet and err if not.
            args.next();

            // The next arg after 'src' loc should be the word 'to'.
            // If it's not, hand back an error:
            let next_is_joiner = if let Some(joiner) = args.next() {
                if joiner.trim() != "to" {
                    false
                } else {
                    true
                }
            } else {
                false
            };
            if !next_is_joiner {
                return Err(err!("Expecting the word 'to' after the location '{}'", peeked));
            }

            // The arg following the 'to' should be another location
            // or something is wrong:
            let dest = if let Some(dest) = args.next() {
                DestLocation::parse(&dest).map_err(|e| {
                    err!("Error parsing '{}': {}", dest, e)
                })
            } else {
                Err(err!("Expecting a destination location to be provided after '{} to'", peeked))
            }?;

            // If we've made it this far, we have a Route:
            routes.push(Route {
                src,
                dest
            });

            // Now, we either break or the next arg is 'and':
            let next_is_and = if let Some(and) = args.peek() {
                if and.trim() != "and" {
                    false
                } else {
                    true
                }
            } else {
                false
            };
            if !next_is_and {
                break
            } else {
                // We expect another valid route now:
                expects_more = true;
                // consume the 'and' if we see it:
                args.next();
            }

        } else {
            // No more Route-like args so break out of this loop:
            break
        }
    }

    // we've seen an 'and', but then failed to parse a location:
    if expects_more {
        return Err(err!("'and' not followed by a subsequent route"));
    }

    // Hand back our routes, plus the rest of the args
    // that we haven't iterated yet, if things were
    // successful:
    Ok(( routes, args ))
}

#[derive(Debug,Clone,PartialEq)]
pub struct Route {
    pub src: SrcLocation,
    pub dest: DestLocation
}

impl Route {
    pub fn src_socket_addr(&self) -> Result<SocketAddr, Error> {
        let mut addrs = self.src.url.to_socket_addrs().map_err(|e| {
            err!("Cannot parse socket address to listen on: {}", e)
        })?;

        if let Some(addr) = addrs.next() {
            Ok(addr)
        } else {
            Err(err!("Cannot parse socket address to listen on"))
        }
    }
}