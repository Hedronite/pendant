//! Pendant's phone app: an nsh shell over the bridge.
//!
//! - [`App`] is the root `nsh.json` names. It holds the navigation stack.
//! - [`net`] talks to the bridge (nsh `fetch`, and the WebSocket envelope).
//! - [`agents`] holds the screens, and [`theme`] the colours.
//! - On the host only, the notes store (and its Lapis link) stays the
//!   library it was, so `pendant_app::{Config, Store}` and its tests are
//!   unchanged. The phone builds have no Lapis link until D1.

pub mod agents;
pub mod net;
pub mod theme;

#[path = "platform/d1.rs"]
mod d1;
/// The D1 folder probe (G3): the phone's folder picker and what it gives.
#[path = "platform/folders.rs"]
pub mod folders;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
#[path = "notes/store.rs"]
mod store;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use store::*;

// On the phones the store is not built, so the device key mounts here.
#[cfg(any(target_os = "ios", target_os = "android"))]
#[path = "platform/keystore.rs"]
pub mod keystore;

use nsh_base::nav_stack;
use nsh_core::prelude::*;

/// The app's UI set. `nsh package` registers it in the runner.
#[native_module("pendant/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// A page on the navigation stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The agent list: the bottom page.
    Agents,
    /// One agent, by the id the bridge gives it.
    Agent(String),
    /// Blocked and done events.
    Inbox,
    /// Pairing: the payload when a `pendant://pair` link brought one, else
    /// the screen asks for it to be pasted. `Payload` never prints itself.
    Pair(Option<net::pair::Payload>),
    /// The D1 folder probe (`pendant://d1`).
    D1,
}

impl Route {
    /// The page's key on the stack. Unique for each route.
    pub fn key(&self) -> String {
        match self {
            Self::Agents => "agents".to_owned(),
            Self::Agent(id) => format!("agent:{id}"),
            Self::Inbox => "inbox".to_owned(),
            // One pairing page, whatever brought it: the key never holds
            // the payload.
            Self::Pair(_) => "pair".to_owned(),
            Self::D1 => "d1".to_owned(),
        }
    }
}

/// What a screen uses to move: push a page, or go back one.
#[derive(Clone, Copy)]
pub struct Nav {
    routes: Signal<Vec<Route>>,
}

impl Nav {
    pub fn push(&self, route: Route) {
        self.routes.update(|routes| {
            // A page already on the stack is not stacked twice.
            if let Some(at) = routes.iter().position(|open| open == &route) {
                routes.truncate(at + 1);
            } else {
                routes.push(route);
            }
        });
    }

    /// Back one page; the agent list always stays.
    pub fn back(&self) {
        self.routes.update(|routes| {
            if routes.len() > 1 {
                routes.pop();
            }
        });
    }

    pub fn depth(&self) -> usize {
        self.routes.with(Vec::len)
    }
}

/// The bridge this build talks to (`PENDANT_BRIDGE_URL` at build time).
pub fn bridge() -> Result<net::Bridge, net::ConfigError> {
    net::Bridge::from_build()
}

/// The variable naming a page to open over the list at launch.
pub const START_VAR: &str = "PENDANT_START";

/// The stack at launch: the agent list, plus the page `start` names
/// (`inbox` or `agent:<id>`), read from [`START_VAR`] at run time. A
/// simulator launch passes it as `SIMCTL_CHILD_PENDANT_START`, so a check
/// can open a page without taps. Anything else opens the list alone.
pub fn start_routes(start: Option<&str>) -> Vec<Route> {
    let mut routes = vec![Route::Agents];
    match start.map(str::trim) {
        Some("inbox") => routes.push(Route::Inbox),
        Some("pair") => routes.push(Route::Pair(None)),
        Some("d1") => routes.push(Route::D1),
        Some(other) => {
            if let Some(id) = other.strip_prefix("agent:").filter(|id| !id.is_empty()) {
                routes.push(Route::Agent(id.to_owned()));
            }
        }
        None => {}
    }
    routes
}

/// The URL scheme the app claims (`nsh.json` `schemes`).
pub const SCHEME: &str = "pendant";

/// The payload in a `pendant://pair?code=<payload>` link, percent-decoded
/// and checked. Anything else is `None`.
pub fn pair_link(url: &str) -> Option<net::pair::Payload> {
    let query = url.trim().strip_prefix("pendant://pair?")?;
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))?;
    net::pair::Payload::parse(&percent_decode(code)?).ok()
}

/// `%XX` decoding for a link's query value; `None` for a broken escape.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Whether `url` is the D1 probe link, `pendant://d1`.
pub fn is_d1_link(url: &str) -> bool {
    url.strip_prefix(SCHEME)
        .and_then(|rest| rest.strip_prefix("://"))
        .is_some_and(|rest| rest.trim_end_matches('/') == "d1")
}

/// The root component.
#[component]
pub fn App() -> impl IntoElement {
    // The live link: snapshot and inbox polling, and the WebSocket worker.
    // Screens read it with `net::live::live()`.
    net::live::start();
    let start = std::env::var(START_VAR).ok();
    let routes = signal(start_routes(start.as_deref()));
    // Links that open the app: `pendant://pair?code=<payload>` opens the
    // pairing page with the payload. Other links are ignored.
    spawn(async move {
        loop {
            let url = next_url_scheme_open().await;
            if let Some(payload) = pair_link(&url) {
                net::live::trace("link: pairing payload received");
                Nav { routes }.push(Route::Pair(Some(payload)));
            } else if is_d1_link(&url) {
                net::live::trace("link: d1 probe");
                Nav { routes }.push(Route::D1);
            } else {
                net::live::trace("link: ignored");
            }
        }
    });
    let nav = Nav { routes };
    nav_stack().flex_1().child(each(
        move || routes.get(),
        |route: &Route| route.key(),
        move |route, index| {
            let page = match route.get_untracked() {
                Route::Agents => agents::list(nav),
                Route::Agent(id) => agents::detail(nav, id),
                Route::Inbox => agents::inbox(nav),
                Route::Pair(payload) => agents::pairing(nav, payload),
                Route::D1 => d1::d1(nav),
            };
            // The user went back natively (swipe, Android back): drop the
            // page and any above it.
            page.on_dismiss(move || {
                let at = index.get_untracked().max(1);
                net::live::trace(&format!("dismissed page {at}"));
                routes.update(|r| r.truncate(at));
            })
        },
    ))
}

#[cfg(test)]
mod d1_link_tests {
    use super::is_d1_link;

    #[test]
    fn only_the_d1_link_opens_the_probe() {
        assert!(is_d1_link("pendant://d1"));
        assert!(is_d1_link("pendant://d1/"));
        for no in [
            "pendant://d1x",
            "pendant://pair?code=x",
            "other://d1",
            "pendant:d1",
            "d1",
        ] {
            assert!(!is_d1_link(no), "{no}");
        }
    }
}

#[cfg(test)]
mod start_tests {
    use super::{start_routes, Route};

    #[test]
    fn the_start_variable_opens_one_page_over_the_list() {
        assert_eq!(start_routes(None), [Route::Agents]);
        assert_eq!(start_routes(Some("inbox")), [Route::Agents, Route::Inbox]);
        assert_eq!(
            start_routes(Some("agent:w1:p2")),
            [Route::Agents, Route::Agent("w1:p2".into())]
        );
        assert_eq!(
            start_routes(Some("pair")),
            [Route::Agents, Route::Pair(None)]
        );
        assert_eq!(start_routes(Some("d1")), [Route::Agents, Route::D1]);
        for ignored in ["", "agent:", "agents", "settings", "agent"] {
            assert_eq!(start_routes(Some(ignored)), [Route::Agents], "{ignored}");
        }
    }
}

#[cfg(test)]
mod link_tests {
    use super::{pair_link, Route};

    const NONCE: &str = "00112233445566778899aabbccddeeff";

    #[test]
    fn a_pair_link_carries_the_payload() {
        let plain = format!("pendant://pair?code=pendant-pair:v1:{NONCE}");
        let escaped = format!("pendant://pair?code=pendant-pair%3Av1%3A{NONCE}");
        let want = crate::net::pair::Payload::parse(&format!("pendant-pair:v1:{NONCE}")).unwrap();
        assert_eq!(pair_link(&plain), Some(want.clone()));
        assert_eq!(pair_link(&escaped), Some(want.clone()));
        assert_eq!(
            pair_link(&format!("pendant://pair?x=1&code=pendant-pair:v1:{NONCE}")),
            Some(want)
        );
        for bad in [
            "pendant://pair",
            "pendant://pair?code=",
            "pendant://pair?code=%zz",
            "pendant://agents?code=x",
            "https://pair?code=x",
        ] {
            assert_eq!(pair_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_pair_route_never_prints_or_keys_its_payload() {
        let payload =
            crate::net::pair::Payload::parse(&format!("pendant-pair:v1:{NONCE}")).unwrap();
        let route = Route::Pair(Some(payload));
        assert_eq!(route.key(), "pair");
        assert!(!format!("{route:?}").contains(NONCE));
    }
}
